const prefersReducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
const layers = Array.from(document.querySelectorAll("[data-depth]"));

function updateParallax() {
  if (prefersReducedMotion.matches) return;

  const scrollY = window.scrollY;
  const viewportHeight = window.innerHeight || 1;
  const normalized = scrollY / viewportHeight;

  layers.forEach((layer) => {
    const depth = Number(layer.dataset.depth || 0);
    const translateY = normalized * depth * -56;
    const translateX = normalized * depth * 18;
    layer.style.transform = `translate3d(${translateX}px, ${translateY}px, 0)`;
  });
}

let frameRequested = false;

function requestParallaxFrame() {
  if (frameRequested) return;
  frameRequested = true;

  requestAnimationFrame(() => {
    frameRequested = false;
    updateParallax();
  });
}

if (!prefersReducedMotion.matches) {
  updateParallax();
  window.addEventListener("scroll", requestParallaxFrame, { passive: true });
  window.addEventListener("resize", requestParallaxFrame);
}
