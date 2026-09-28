// Decrypts the blocks rs-web's rs.crypt.encrypt_html writes: the key is Argon2id (hash-wasm) over the
// block's salt, with rs-web's parameters, and the content is AES-256-GCM (Web Crypto) with its nonce.
(function () {
  "use strict";
  const KEY = "rkb-site-password";
  const b64 = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));

  async function open(block, password) {
    const raw = await hashwasm.argon2id({
      password,
      salt: b64(block.dataset.salt),
      parallelism: 1,
      iterations: 3,
      memorySize: 65536,
      hashLength: 32,
      outputType: "binary",
    });
    const key = await crypto.subtle.importKey("raw", raw, "AES-GCM", false, ["decrypt"]);
    const plain = await crypto.subtle.decrypt({ name: "AES-GCM", iv: b64(block.dataset.nonce) }, key, b64(block.dataset.encrypted));
    const div = document.createElement("div");
    div.className = "decrypted";
    div.innerHTML = new TextDecoder().decode(plain);
    block.replaceWith(div);
  }

  function setup(block) {
    const input = block.querySelector("input[type=password]");
    const button = block.querySelector("button");
    const message = block.querySelector(".encrypted-message");
    const remember = block.querySelector("input.remember");
    if (remember) {
      remember.parentElement.lastChild.textContent = " Remember until this tab closes";
    }
    const go = async () => {
      message.textContent = "Decrypting...";
      try {
        await open(block, input.value);
        if (remember && remember.checked) {
          sessionStorage.setItem(KEY, input.value);
        }
      } catch (e) {
        message.textContent = "Wrong password.";
        input.select();
      }
    };
    button.addEventListener("click", go);
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        go();
      }
    });
  }

  document.addEventListener("DOMContentLoaded", () => {
    const blocks = document.querySelectorAll(".encrypted-content");
    if (!window.crypto || !crypto.subtle || !window.WebAssembly) {
      blocks.forEach((b) => {
        b.querySelector(".encrypted-message").textContent = "This browser cannot decrypt: it needs Web Crypto (https or localhost) and WebAssembly.";
      });
      return;
    }
    const saved = sessionStorage.getItem(KEY);
    blocks.forEach((block) => {
      setup(block);
      if (saved) {
        open(block, saved).catch(() => sessionStorage.removeItem(KEY));
      }
    });
  });
})();
