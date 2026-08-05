/* Big-Shot Voting - compiled into the binary (see src/assets.rs).
   Progressive enhancement only: every page works with JS disabled. */
(() => {
  "use strict";

  // --- Lightbox ------------------------------------------------------------
  //
  // One dialog is reused for every tile. Media is created on open and torn
  // down on close, so we never hold decoded frames for entries nobody viewed.

  const dialog = document.getElementById("lightbox");
  if (dialog && typeof dialog.showModal === "function") {
    const stage = dialog.querySelector(".lightbox-stage");
    const caption = dialog.querySelector(".lightbox-caption");
    const openBtn = dialog.querySelector("[data-lb-open]");
    const masterBtn = dialog.querySelector("[data-lb-master]");
    const masterUploadBtn = dialog.querySelector("[data-lb-master-upload]");
    const masterForm = document.getElementById("lightbox-master-form");
    const masterFile = masterForm
      ? masterForm.querySelector("[data-lb-master-file]")
      : null;

    /** @type {HTMLElement[]} */
    let triggers = [];
    let index = -1;
    /** @type {string} */
    let masterUploadAction = "";

    const refresh = () => {
      triggers = Array.from(document.querySelectorAll("[data-lightbox]"));
    };

    const clearStage = () => {
      // Stop playback before dropping the element or audio can outlive the DOM.
      stage.querySelectorAll("video, audio").forEach((m) => {
        m.pause();
        m.removeAttribute("src");
        m.load();
      });
      stage.replaceChildren();
    };

    const render = (i) => {
      const el = triggers[i];
      if (!el) return;
      index = i;

      const src = el.dataset.src;
      const kind = el.dataset.kind || "file";
      const title = el.dataset.title || "Entry";
      const author = el.dataset.author || "";
      const master = el.dataset.master || "";
      const canUploadMaster = el.dataset.masterUpload || "";

      clearStage();

      let node;
      if (kind === "image") {
        node = document.createElement("img");
        node.src = src;
        node.alt = title;
      } else if (kind === "video") {
        node = document.createElement("video");
        node.src = src;
        node.controls = true;
        node.autoplay = true;
        node.playsInline = true;
      } else if (kind === "audio") {
        node = document.createElement("audio");
        node.src = src;
        node.controls = true;
        node.autoplay = true;
      } else {
        node = document.createElement("div");
        node.className = "lightbox-file";
        node.textContent = title;
      }
      stage.append(node);

      caption.innerHTML = "";
      const strong = document.createElement("strong");
      strong.textContent = title;
      caption.append(strong);
      if (author) caption.append(document.createTextNode(` by ${author}`));

      if (openBtn) openBtn.href = src;

      masterUploadAction = "";
      if (masterBtn) {
        if (master) {
          masterBtn.href = master;
          masterBtn.hidden = false;
        } else {
          masterBtn.removeAttribute("href");
          masterBtn.hidden = true;
        }
      }
      if (masterUploadBtn) {
        // Same slot as download: only the owner of an entry without a master sees this.
        if (!master && canUploadMaster) {
          masterUploadAction = canUploadMaster;
          masterUploadBtn.hidden = false;
        } else {
          masterUploadBtn.hidden = true;
        }
      }
    };

    document.addEventListener("click", (event) => {
      const trigger = event.target.closest("[data-lightbox]");
      if (!trigger) return;
      event.preventDefault();
      refresh();
      render(triggers.indexOf(trigger));
      if (!dialog.open) dialog.showModal();
    });

    dialog.querySelector("[data-lb-close]").addEventListener("click", () => dialog.close());

    if (masterUploadBtn && masterFile && masterForm) {
      masterUploadBtn.addEventListener("click", () => {
        if (!masterUploadAction) return;
        masterFile.value = "";
        masterFile.click();
      });
      masterFile.addEventListener("change", () => {
        if (!masterFile.files || !masterFile.files[0] || !masterUploadAction) return;
        masterForm.action = masterUploadAction;
        masterUploadBtn.disabled = true;
        masterUploadBtn.textContent = masterForm.dataset.busy || "Uploading…";
        masterForm.submit();
      });
    }

    // Click outside the frame (i.e. on the backdrop) closes.
    dialog.addEventListener("click", (event) => {
      if (event.target === dialog) dialog.close();
    });

    dialog.addEventListener("close", () => {
      clearStage();
      const trigger = triggers[index];
      if (trigger) trigger.focus({ preventScroll: true });
    });
  }

  // --- Upload form niceties ------------------------------------------------

  document.querySelectorAll("[data-file-input]").forEach((input) => {
    const label = document.querySelector(input.dataset.fileInput);
    if (!label) return;
    input.addEventListener("change", () => {
      const file = input.files && input.files[0];
      label.textContent = file
        ? `${file.name} (${(file.size / 1048576).toFixed(1)} MB)`
        : "";
    });
  });

  // Guard against double submits on uploads (large files take a while).
  document.querySelectorAll("form[data-busy]").forEach((form) => {
    form.addEventListener("submit", () => {
      const button = form.querySelector("button[type=submit]");
      if (!button) return;
      // Defer so the button's value still makes it into the POST body.
      setTimeout(() => {
        button.disabled = true;
        button.textContent = form.dataset.busy;
      }, 0);
    });
  });

  // --- Flash toast ---------------------------------------------------------
  // Success messages after redirects render as a fixed toast (no layout shift).
  // Without JS the toast still shows; with JS it auto-dismisses.

  document.querySelectorAll("[data-toast]").forEach((toast) => {
    const dismiss = () => {
      toast.classList.add("is-leaving");
      const remove = () => toast.remove();
      toast.addEventListener("animationend", remove, { once: true });
      // Fallback if animation is disabled or never fires.
      setTimeout(remove, 400);
    };
    setTimeout(dismiss, 4200);
  });
})();
