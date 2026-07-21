// SPDX-License-Identifier: GPL-3.0-or-later
// Mock chrome: theme toggle (system → light → dark), persisted per browser.
(function () {
  const KEY = 'vh-mock-theme';
  const root = document.documentElement;
  const order = ['system', 'light', 'dark'];
  const label = { system: 'theme: system', light: 'theme: light', dark: 'theme: dark' };

  function apply(mode) {
    if (mode === 'system') root.removeAttribute('data-theme');
    else root.setAttribute('data-theme', mode);
    const btn = document.querySelector('.theme-toggle');
    if (btn) btn.textContent = label[mode];
  }

  let mode = localStorage.getItem(KEY) || 'system';
  document.addEventListener('DOMContentLoaded', function () {
    apply(mode);
    // follow-tail depiction: logs open pinned to the newest line
    document.querySelectorAll('.log').forEach(function (el) {
      el.scrollTop = el.scrollHeight;
    });
    const btn = document.querySelector('.theme-toggle');
    if (btn)
      btn.addEventListener('click', function () {
        mode = order[(order.indexOf(mode) + 1) % order.length];
        localStorage.setItem(KEY, mode);
        apply(mode);
      });
  });
  if (mode !== 'system') root.setAttribute('data-theme', mode);
})();
