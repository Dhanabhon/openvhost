// SPDX-License-Identifier: GPL-3.0-or-later
// Mock chrome helpers.
document.addEventListener('DOMContentLoaded', function () {
  // follow-tail depiction: logs open pinned to the newest line
  document.querySelectorAll('.log').forEach(function (el) {
    el.scrollTop = el.scrollHeight;
  });
});
