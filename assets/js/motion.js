// reused-from: orchos-site@6d2f0a2:src/motion/motion.js (2026-11-16) — adapted: the gate only (fx / jsfx classes on <html>); the binding engine is replaced by a small IntersectionObserver that reveals `.reveal` blocks
//
// The gate sets, on <html>:
//   fx    when JavaScript runs and the visitor has not asked for reduced motion.
//         Every motion rule in style.css lives under it, so with no JavaScript,
//         or with reduced motion, the page is static and complete.
//   jsfx  when the browser lacks CSS scroll-driven animations. Then the
//         observer below adds `.in` to each `.reveal` block as it scrolls in.
// It never intercepts scrolling. Loaded in <head> so it runs before first paint.
(function () {
  var d = document.documentElement;
  var w = window;
  var reduce = w.matchMedia ? w.matchMedia("(prefers-reduced-motion: reduce)") : null;
  var sda = false;
  try {
    sda = CSS.supports("animation-timeline: view()");
  } catch (e) {}

  function apply() {
    d.classList.toggle("fx", !(reduce && reduce.matches));
    d.classList.toggle("jsfx", !sda);
  }
  try {
    apply();
  } catch (e) {
    return;
  }
  if (reduce && reduce.addEventListener) reduce.addEventListener("change", apply);
  if (sda) return;
  if (!("IntersectionObserver" in w)) {
    // No way to reveal on scroll: show everything.
    d.classList.remove("fx");
    return;
  }

  var io = new IntersectionObserver(
    function (entries) {
      entries.forEach(function (entry) {
        if (!entry.isIntersecting) return;
        entry.target.classList.add("in");
        io.unobserve(entry.target);
      });
    },
    { rootMargin: "0px 0px -8% 0px" }
  );
  function scan() {
    document.querySelectorAll(".reveal:not(.in)").forEach(function (el) {
      io.observe(el);
    });
  }
  w.addEventListener("DOMContentLoaded", function () {
    scan();
    // The app replaces the view's HTML as data arrives; watch for new blocks.
    var view = document.getElementById("view");
    if (view) new MutationObserver(scan).observe(view, { childList: true, subtree: true });
  });
})();
