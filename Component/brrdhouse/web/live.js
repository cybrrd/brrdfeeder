// SPDX-License-Identifier: AGPL-3.0-or-later
(() => {
  'use strict';
  const status = document.getElementById('live-status');
  const warning = document.getElementById('connection-warning');
  document.getElementById('local-address').textContent = window.location.host;
  let lastMessage = performance.now();
  const lost = () => { status.hidden = true; warning.hidden = false; };
  document.body.addEventListener('htmx:sseError', lost);
  document.body.addEventListener('htmx:sseMessage', () => {
    lastMessage = performance.now();
    status.hidden = false;
    warning.hidden = true;
  });
  // Recheck when a sleeping/backgrounded browser returns, before trusting old DOM.
  const check = () => { if (performance.now() - lastMessage > 4000) lost(); };
  setInterval(check, 1000);
  document.addEventListener('visibilitychange', check);
  window.addEventListener('pageshow', check);
})();
