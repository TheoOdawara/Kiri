const viewer = document.createElement("dialog");
viewer.className = "image-viewer";
viewer.setAttribute("aria-label", "Enlarged image");
viewer.innerHTML = `
  <div class="image-viewer__stage"><img class="image-viewer__picture" alt="" draggable="false"></div>
  <div class="image-viewer__bar">
    <span class="image-viewer__hint">Scroll to zoom, drag to move, double-click to fit</span>
    <button class="image-viewer__button" type="button" data-action="fit">Fit</button>
    <button class="image-viewer__button" type="button" data-action="close">Close</button>
  </div>`;
document.body.append(viewer);

const stage = viewer.querySelector(".image-viewer__stage");
const picture = viewer.querySelector(".image-viewer__picture");
const view = { width: 0, left: 0, top: 0, fitWidth: 0 };

const heightRatio = () => picture.naturalHeight / picture.naturalWidth;

const draw = () => {
  picture.style.width = `${view.width}px`;
  picture.style.transform = `translate(${view.left}px, ${view.top}px)`;
};

const fit = () => {
  const margin = 0.94;
  view.fitWidth = Math.min(stage.clientWidth * margin, (stage.clientHeight * margin) / heightRatio());
  view.width = view.fitWidth;
  view.left = (stage.clientWidth - view.width) / 2;
  view.top = (stage.clientHeight - view.width * heightRatio()) / 2;
  draw();
};

picture.addEventListener("load", fit);
window.addEventListener("resize", () => {
  if (viewer.open) fit();
});

document.addEventListener("click", (event) => {
  const image = event.target.closest(".md-content img");
  if (!image || image.closest("a")) return;
  picture.alt = image.alt;
  picture.src = image.currentSrc;
  viewer.showModal();
  if (picture.complete) fit();
});

viewer.addEventListener("click", (event) => {
  const action = event.target.closest("[data-action]")?.dataset.action;
  if (action === "fit") fit();
  if (action === "close") viewer.close();
});

stage.addEventListener("dblclick", fit);

stage.addEventListener(
  "wheel",
  (event) => {
    event.preventDefault();
    const sensitivity = event.ctrlKey ? 0.01 : 0.0015;
    const wanted = view.width * Math.exp(-event.deltaY * sensitivity);
    const width = Math.min(Math.max(wanted, view.fitWidth * 0.5), view.fitWidth * 24);
    const bounds = stage.getBoundingClientRect();
    const pointerX = event.clientX - bounds.left;
    const pointerY = event.clientY - bounds.top;
    const growth = width / view.width;
    view.left = pointerX - (pointerX - view.left) * growth;
    view.top = pointerY - (pointerY - view.top) * growth;
    view.width = width;
    draw();
  },
  { passive: false },
);

// ponytail: mouse and trackpad only; track two pointers for pinch if the docs get read on a phone
stage.addEventListener("pointerdown", (event) => {
  stage.setPointerCapture(event.pointerId);
  stage.dataset.dragging = "";
});

stage.addEventListener("pointermove", (event) => {
  if (!stage.hasPointerCapture(event.pointerId)) return;
  view.left += event.movementX;
  view.top += event.movementY;
  draw();
});

stage.addEventListener("pointerup", (event) => {
  stage.releasePointerCapture(event.pointerId);
  delete stage.dataset.dragging;
});
