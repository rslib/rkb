//! The pinned model files: where they live, how they are checked, and how they are fetched.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use sha2::{Digest, Sha256};

pub const REPO: &str = "nomic-ai/modernbert-embed-base";
pub const REVISION: &str = "d556a88e332558790b210f7bdbe87da2fa94a8d8";

/// (path in the repository, SHA-256, size in bytes).
pub type File = (&'static str, &'static str, u64);

pub const FILES: [File; 3] = [
    ("model.safetensors", "69f22ccd1ee0971b6173678c4fcadd29a49a3f5b6b8a6927002418abe4bd5b9c", 596_070_136),
    ("tokenizer.json", "6c8aaa9a542084f2457eab775d4eeb51f92a70c0fd9de28d5edb0ddec3c08d30", 3_583_228),
    ("config.json", "dea4708ed1a6df7306b483037750464a96e77ccb0c7e88f4f61a0116944a71dd", 1_261),
];

/// `$XDG_DATA_HOME/rkb/models/modernbert-embed-base`, or `~/.local/share/rkb/models/modernbert-embed-base`.
pub fn default_dir() -> PathBuf {
    let base = match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(v) => PathBuf::from(v),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"),
    };
    base.join("rkb/models/modernbert-embed-base")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hash_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// Records a file's size and modification time when its hash last matched, so a load does not hash 596 MB every time.
fn stamp(path: &Path) -> Option<String> {
    let m = std::fs::metadata(path).ok()?;
    let mtime = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some(format!("{} {mtime}", m.len()))
}

/// Whether one file is present and matches its pinned hash, hashing only when it changed since the last match.
fn verified(dir: &Path, name: &str, sha: &str, size: u64) -> Result<bool> {
    let path = dir.join(name);
    let Some(now) = stamp(&path) else { return Ok(false) };
    if !now.starts_with(&format!("{size} ")) {
        return Ok(false);
    }
    let sp = dir.join(".verified").join(name);
    if std::fs::read_to_string(&sp).is_ok_and(|s| s == now) {
        return Ok(true);
    }
    if hash_file(&path)? != sha {
        return Ok(false);
    }
    std::fs::create_dir_all(sp.parent().expect("under .verified"))?;
    std::fs::write(&sp, now)?;
    Ok(true)
}

/// Fails with the reason when a file is missing or does not match its pinned hash.
pub fn check(dir: &Path) -> Result<()> {
    check_files(dir, &FILES)
}

fn check_files(dir: &Path, files: &[File]) -> Result<()> {
    for &(name, sha, size) in files {
        if !dir.join(name).is_file() {
            bail!("not configured: {name} is missing; run `rkb models fetch`");
        }
        if !verified(dir, name, sha, size)? {
            bail!("model files do not match ({name}); run `rkb models fetch`");
        }
    }
    Ok(())
}

#[derive(Debug, Default)]
pub struct Fetched {
    pub downloaded: Vec<String>,
    pub present: Vec<String>,
    pub bytes: u64,
}

/// Downloads every file that is missing or wrong from `base` (default `https://huggingface.co`) into `dir`.
/// A file is written to `.<name>.part`, hashed while it streams, and renamed only when the hash matches.
pub fn fetch(dir: &Path, base: &str) -> Result<Fetched> {
    fetch_files(dir, base, &FILES)
}

/// Verifies against the OS trust store, so a TLS-intercepting proxy whose CA is installed on the
/// system (but not in webpki roots) stops failing with UnknownIssuer.
fn agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
            .build()
            .into()
    })
}

fn fetch_files(dir: &Path, base: &str, files: &[File]) -> Result<Fetched> {
    let mut out = Fetched::default();
    std::fs::create_dir_all(dir)?;
    for &(name, sha, size) in files {
        if verified(dir, name, sha, size)? {
            out.present.push(name.to_string());
            continue;
        }
        let url = format!("{}/{REPO}/resolve/{REVISION}/{name}", base.trim_end_matches('/'));
        let target = dir.join(name);
        let part = dir.join(format!(".{name}.part"));
        let mut resp = agent().get(&url).call().map_err(|e| anyhow::anyhow!("{url}: {e}"))?;
        let mut reader = resp.body_mut().as_reader();
        let mut file = std::fs::File::create(&part)?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut n_total = 0u64;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            file.write_all(&buf[..n])?;
            n_total += n as u64;
        }
        file.sync_all()?;
        let got = hex(&h.finalize());
        if got != sha {
            let _ = std::fs::remove_file(&part);
            bail!("{name}: downloaded file does not match its pinned hash (got {got})");
        }
        std::fs::rename(&part, &target)?;
        out.bytes += n_total;
        out.downloaded.push(name.to_string());
        verified(dir, name, sha, size)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;

    /// Serves `files` (request path suffix, body) over HTTP on a local port; returns the base URL.
    fn serve(files: Vec<(String, Vec<u8>)>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                    line.clear();
                }
                let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
                let mut s = stream;
                match files.iter().find(|(p, _)| path.ends_with(p.as_str())) {
                    Some((_, body)) => {
                        write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
                        s.write_all(body).unwrap();
                    }
                    None => write!(s, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap(),
                }
            }
        });
        base
    }

    #[test]
    fn fetch_twice_and_a_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (b"weights".to_vec(), b"{\"tokens\": 1}".to_vec());
        let (sa, sb) = (hex(&Sha256::digest(&a)), hex(&Sha256::digest(&b)));
        let files: Vec<File> = vec![
            ("model.safetensors", Box::leak(sa.into_boxed_str()), a.len() as u64),
            ("tokenizer.json", Box::leak(sb.into_boxed_str()), b.len() as u64),
        ];
        let base = serve(vec![("/model.safetensors".into(), a.clone()), ("/tokenizer.json".into(), b.clone())]);
        let e = check_files(dir.path(), &files).unwrap_err().to_string();
        assert!(e.starts_with("not configured"), "{e}");
        let first = fetch_files(dir.path(), &base, &files).unwrap();
        assert_eq!((first.downloaded.len(), first.bytes), (2, (a.len() + b.len()) as u64));
        check_files(dir.path(), &files).unwrap();
        let second = fetch_files(dir.path(), &base, &files).unwrap();
        assert_eq!((second.downloaded.len(), second.present.len()), (0, 2), "nothing to download");

        std::fs::write(dir.path().join("model.safetensors"), b"weightz").unwrap();
        let e = check_files(dir.path(), &files).unwrap_err().to_string();
        assert!(e.contains("do not match") && e.contains("rkb models fetch"), "{e}");
        let again = fetch_files(dir.path(), &base, &files).unwrap();
        assert_eq!(again.downloaded, ["model.safetensors"]);
        check_files(dir.path(), &files).unwrap();
    }

    #[test]
    fn a_wrong_download_is_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<File> = vec![("model.safetensors", "00", 3)];
        let base = serve(vec![("/model.safetensors".into(), b"bad".to_vec())]);
        let e = fetch_files(dir.path(), &base, &files).unwrap_err().to_string();
        assert!(e.contains("does not match"), "{e}");
        assert!(!dir.path().join("model.safetensors").exists());
        assert!(!dir.path().join(".model.safetensors.part").exists());
    }
}
