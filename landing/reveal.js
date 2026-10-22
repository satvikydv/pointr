// Scroll reveals and the reading-progress bar. Content is visible by default;
// the inline script in <head> adds `.js` only when motion is welcome.
(function () {
  'use strict';

  var root = document.documentElement;
  window.__reveal = true;

  // --- Reading progress bar ---
  var bar = document.querySelector('.scroll-progress');
  if (bar) {
    var ticking = false;
    var update = function () {
      var max = root.scrollHeight - window.innerHeight;
      bar.style.transform = 'scaleX(' + (max > 0 ? Math.min(window.scrollY / max, 1) : 0) + ')';
      ticking = false;
    };
    window.addEventListener('scroll', function () {
      if (!ticking) { ticking = true; requestAnimationFrame(update); }
    }, { passive: true });
    update();
  }

  if (!root.classList.contains('js')) return;

  // --- Reveal on scroll ---
  var items = document.querySelectorAll('.reveal');

  // Stagger the changelog entries that arrive together.
  document.querySelectorAll('.changelog-item.reveal').forEach(function (el, i) {
    if (i < 4) el.style.setProperty('--reveal-delay', (i * 0.07) + 's');
  });

  // Once shown, drop the helper classes so they can't interfere with hover transitions.
  function settle(el) {
    var done = function (e) {
      if (e.propertyName !== 'opacity') return;
      el.removeEventListener('transitionend', done);
      el.classList.remove('reveal', 'from-left', 'from-right', 'is-visible');
      el.style.removeProperty('--reveal-delay');
    };
    el.addEventListener('transitionend', done);
  }

  var io = new IntersectionObserver(function (entries) {
    entries.forEach(function (entry) {
      if (!entry.isIntersecting) return;
      settle(entry.target);
      entry.target.classList.add('is-visible');
      io.unobserve(entry.target);
    });
  }, { threshold: 0.15, rootMargin: '0px 0px -8% 0px' });

  items.forEach(function (el) { io.observe(el); });
})();
