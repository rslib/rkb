//! Removes metadata from PNG, JPEG and WebP images by keeping only the blocks that draw the image.
//! An allow list, so EXIF, XMP, IPTC, ICC profiles, comments, text chunks and unknown blocks all go.

/// The image without metadata, or why the file could not be read as `ext`.
pub fn strip(ext: &str, data: &[u8]) -> Result<Vec<u8>, String> {
    match ext.to_ascii_lowercase().as_str() {
        "png" => png(data),
        "jpg" | "jpeg" => jpeg(data),
        "webp" => webp(data),
        other => Err(format!("no metadata removal for .{other} files")),
    }
}

const PNG_SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
/// Critical chunks, transparency, colour basics and APNG animation.
const PNG_KEEP: [&[u8; 4]; 12] =
    [b"IHDR", b"PLTE", b"IDAT", b"IEND", b"tRNS", b"gAMA", b"cHRM", b"sRGB", b"sBIT", b"acTL", b"fcTL", b"fdAT"];

fn png(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut rest = data.strip_prefix(PNG_SIG).ok_or("not a PNG file")?;
    let mut out = PNG_SIG.to_vec();
    loop {
        let len = u32::from_be_bytes(rest.get(..4).ok_or("truncated PNG")?.try_into().expect("4 bytes")) as usize;
        let chunk = rest.get(..12 + len).ok_or("truncated PNG chunk")?;
        let kind = &chunk[4..8];
        if PNG_KEEP.iter().any(|k| &k[..] == kind) {
            out.extend_from_slice(chunk);
        }
        if kind == b"IEND" {
            return Ok(out);
        }
        rest = &rest[12 + len..];
    }
}

fn jpeg(data: &[u8]) -> Result<Vec<u8>, String> {
    if !data.starts_with(&[0xFF, 0xD8]) {
        return Err("not a JPEG file".into());
    }
    let mut out = vec![0xFF, 0xD8];
    let mut i = 2;
    loop {
        while data.get(i) == Some(&0xFF) && data.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        if data.get(i) != Some(&0xFF) {
            return Err(format!("no JPEG marker at byte {i}"));
        }
        let marker = *data.get(i + 1).ok_or("truncated JPEG")?;
        if marker == 0xD9 {
            out.extend_from_slice(&[0xFF, 0xD9]);
            return Ok(out);
        }
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            out.extend_from_slice(&data[i..i + 2]);
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes(data.get(i + 2..i + 4).ok_or("truncated JPEG")?.try_into().expect("2 bytes")) as usize;
        let seg = data.get(i..i + 2 + len).ok_or("truncated JPEG segment")?;
        let body = &seg[4..];
        let keep = match marker {
            0xE0 => body.starts_with(b"JFIF\0"),
            0xEE => body.starts_with(b"Adobe"),
            0xE1..=0xEF | 0xFE => false,
            _ => true,
        };
        if keep {
            out.extend_from_slice(seg);
        }
        i += 2 + len;
        if marker == 0xDA {
            // Entropy-coded data runs to the next marker that is not a stuffed 0xFF00 or a restart.
            let start = i;
            while i + 1 < data.len() && !(data[i] == 0xFF && data[i + 1] != 0 && !(0xD0..=0xD7).contains(&data[i + 1])) {
                i += 1;
            }
            if i + 1 >= data.len() {
                return Err("truncated JPEG scan".into());
            }
            out.extend_from_slice(&data[start..i]);
        }
    }
}

fn webp(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return Err("not a WebP file".into());
    }
    let mut out = b"RIFF\0\0\0\0WEBP".to_vec();
    let mut i = 12;
    while i < data.len() {
        let head = data.get(i..i + 8).ok_or("truncated WebP chunk")?;
        let len = u32::from_le_bytes(head[4..8].try_into().expect("4 bytes")) as usize;
        let chunk = data.get(i..i + 8 + len).ok_or("truncated WebP chunk")?;
        match &head[..4] {
            b"VP8 " | b"VP8L" | b"ALPH" | b"ANIM" | b"ANMF" => out.extend_from_slice(chunk),
            b"VP8X" => {
                let start = out.len();
                out.extend_from_slice(chunk);
                // Flags: ICC 0x20, EXIF 0x08 and XMP 0x04 name chunks that are gone now.
                if len > 0 {
                    out[start + 8] &= !(0x20 | 0x08 | 0x04);
                }
            }
            _ => {}
        }
        if len % 2 == 1 {
            out.push(0);
        }
        i += 8 + len + len % 2;
    }
    let size = u32::try_from(out.len() - 8).map_err(|_| "WebP file too large")?;
    out[4..8].copy_from_slice(&size.to_le_bytes());
    Ok(out)
}

/// Standard base64 with padding.
pub fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = u32::from(c[0]) << 16 | u32::from(*c.get(1).unwrap_or(&0)) << 8 | u32::from(*c.get(2).unwrap_or(&0));
        for k in 0..4 {
            out.push(if k <= c.len() { A[(n >> (18 - 6 * k) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        c.extend_from_slice(kind);
        c.extend_from_slice(data);
        c.extend_from_slice(&[0; 4]);
        c
    }

    /// A 1x1 PNG with GPS EXIF, a text chunk and an ICC profile.
    pub(crate) fn png_with_gps() -> Vec<u8> {
        let mut p = PNG_SIG.to_vec();
        p.extend(png_chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0]));
        p.extend(png_chunk(b"eXIf", b"MM\0*GPSLatitude 41.8781"));
        p.extend(png_chunk(b"tEXt", b"Author\0someone"));
        p.extend(png_chunk(b"iCCP", b"profile"));
        p.extend(png_chunk(b"IDAT", &[0x78, 0x9c, 0x63, 0x60, 0, 0, 0, 2, 0, 1]));
        p.extend(png_chunk(b"IEND", &[]));
        p
    }

    #[test]
    fn png_keeps_only_image_chunks() {
        let out = strip("PNG", &png_with_gps()).unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("IHDR") && text.contains("IDAT") && text.ends_with("IEND\0\0\0\0"), "{text}");
        for gone in ["eXIf", "GPS", "tEXt", "someone", "iCCP"] {
            assert!(!text.contains(gone), "{gone} survived");
        }
        assert!(strip("png", b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR").is_err(), "truncated");
        assert!(strip("png", b"GIF89a").is_err());
    }

    #[test]
    fn jpeg_drops_app_segments_and_comments() {
        let seg = |m: u8, body: &[u8]| {
            let mut s = vec![0xFF, m];
            s.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
            s.extend_from_slice(body);
            s
        };
        let mut j = vec![0xFF, 0xD8];
        j.extend(seg(0xE0, b"JFIF\0\x01\x01"));
        j.extend(seg(0xE1, b"Exif\0\0GPS"));
        j.extend(seg(0xE1, b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta/>"));
        j.extend(seg(0xED, b"Photoshop 3.0\0IPTC"));
        j.extend(seg(0xFE, b"a comment"));
        j.extend(seg(0xDB, &[0; 5]));
        j.extend(seg(0xDA, &[1, 2, 3]));
        j.extend([0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56]);
        j.extend(seg(0xE1, b"Exif\0\0late"));
        j.extend([0xFF, 0xD9]);
        j.extend(b"trailing");
        let out = strip("jpg", &j).unwrap();
        let text = String::from_utf8_lossy(&out);
        for gone in ["Exif", "GPS", "xmpmeta", "IPTC", "comment", "late", "trailing"] {
            assert!(!text.contains(gone), "{gone} survived");
        }
        assert!(text.contains("JFIF"));
        assert!(out.windows(7).any(|w| w == [0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56]), "scan data kept whole");
        assert!(out.ends_with(&[0xFF, 0xD9]));
    }

    #[test]
    fn webp_drops_exif_and_xmp_and_clears_flags() {
        let chunk = |k: &[u8; 4], d: &[u8]| {
            let mut c = k.to_vec();
            c.extend_from_slice(&(d.len() as u32).to_le_bytes());
            c.extend_from_slice(d);
            if d.len() % 2 == 1 {
                c.push(0);
            }
            c
        };
        let mut body = b"WEBP".to_vec();
        body.extend(chunk(b"VP8X", &[0x2C, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        body.extend(chunk(b"VP8L", b"pixels!"));
        body.extend(chunk(b"EXIF", b"GPS"));
        body.extend(chunk(b"XMP ", b"<x:xmpmeta/>"));
        let mut w = b"RIFF".to_vec();
        w.extend_from_slice(&(body.len() as u32).to_le_bytes());
        w.extend(body);
        let out = strip("webp", &w).unwrap();
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("VP8L") && text.contains("pixels!") && !text.contains("GPS") && !text.contains("xmpmeta"), "{text}");
        assert_eq!(out[20], 0, "VP8X flags cleared");
        assert_eq!(u32::from_le_bytes(out[4..8].try_into().unwrap()) as usize, out.len() - 8);
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xFB, 0xFF]), "+/8=");
    }
}
