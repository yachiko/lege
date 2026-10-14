// lector's viewer: in-page navigation, live reload, sidebar and keys.
// The page URL is always the open document's path; see src/server.rs.
(() => {
  'use strict';

  const MARKDOWN = /\.(md|markdown|mdown|mkd)$/i;
  const POLL_MS = 500;

  const body = document.body;
  const page = document.getElementById('page');
  const sidebar = document.getElementById('sidebar');
  const resizer = document.getElementById('resizer');

  // Per-viewer preferences; storage can be unavailable, so never rely on it.
  const prefs = {
    get(key, fallback) {
      try {
        const value = localStorage.getItem(key);
        return value === null ? fallback : JSON.parse(value);
      } catch {
        return fallback;
      }
    },
    set(key, value) {
      try { localStorage.setItem(key, JSON.stringify(value)); } catch {}
    },
  };

  let mtime = body.dataset.mtime;
  let shownPath = location.pathname;

  const decode = (path) => {
    try { return decodeURIComponent(path); } catch { return path; }
  };
  const samePath = (a, b) => decode(a) === decode(b);
  const hasDocument = () => MARKDOWN.test(decode(location.pathname));

  // ---------- Documents ----------

  async function show(url, { push = false, scroll = null } = {}) {
    let response;
    try {
      response = await fetch(url.pathname + '?content');
    } catch {
      return;
    }
    if (!response.ok) return;
    const html = await response.text();

    if (push) {
      history.replaceState({ scroll: window.scrollY }, '');
      history.pushState({}, '', url.pathname + url.hash);
    }
    page.innerHTML = html;
    shownPath = url.pathname;
    const name = decode(url.pathname).split('/').pop();
    document.title = `${name} — lector`;
    mtime = await fetchMtime();

    if (scroll !== null) window.scrollTo(0, scroll);
    else if (url.hash) scrollToHash(url.hash);
    else window.scrollTo(0, 0);
    markCurrent(true);
  }

  async function fetchMtime() {
    try {
      const response = await fetch(location.pathname + '?mtime');
      return response.ok ? await response.text() : mtime;
    } catch {
      return mtime;
    }
  }

  function scrollToHash(hash) {
    const target = document.getElementById(decode(hash.slice(1)));
    if (target) target.scrollIntoView();
  }

  // Live reload: re-render in place when the file changes on disk.
  setInterval(async () => {
    if (!hasDocument() || document.hidden) return;
    const latest = await fetchMtime();
    if (latest && latest !== mtime) {
      show(new URL(location.href), { scroll: window.scrollY });
    }
  }, POLL_MS);

  document.addEventListener('click', (event) => {
    if (event.defaultPrevented || event.button !== 0) return;
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const link = event.target.closest('a[href]');
    if (!link) return;
    const url = new URL(link.href, location.href);

    // Other schemes and non-Markdown files fall through to the native
    // navigation, which lector hands to the system's default app.
    if (url.origin !== location.origin || !MARKDOWN.test(decode(url.pathname))) return;
    // Same-document #anchors scroll natively.
    if (samePath(url.pathname, location.pathname) && url.hash) return;

    event.preventDefault();
    if (!samePath(url.pathname, location.pathname)) show(url, { push: true });
  });

  window.addEventListener('popstate', (event) => {
    if (samePath(location.pathname, shownPath)) {
      if (location.hash) scrollToHash(location.hash);
      return;
    }
    show(new URL(location.href), { scroll: event.state?.scroll ?? null });
  });

  // ---------- Sidebar ----------

  function markCurrent(reveal) {
    let current = null;
    for (const link of sidebar.querySelectorAll('a[href]')) {
      const isCurrent = samePath(new URL(link.href).pathname, location.pathname);
      link.toggleAttribute('aria-current', isCurrent);
      if (isCurrent) current = link;
    }
    if (!current || !reveal) return;
    for (let el = current.parentElement; el && el !== sidebar; el = el.parentElement) {
      if (el.tagName === 'DETAILS') el.open = true;
    }
    // Scroll only the sidebar; scrollIntoView would also move the document.
    const top = current.offsetTop; // relative to the sidebar, its offsetParent
    if (top < sidebar.scrollTop || top + current.offsetHeight > sidebar.scrollTop + sidebar.clientHeight) {
      sidebar.scrollTop = top - sidebar.clientHeight / 3;
    }
  }

  // Pick up added or removed files when coming back to the window,
  // keeping the folders that were open.
  async function refreshTree() {
    if (!body.classList.contains('has-sidebar')) return;
    let html;
    try {
      const response = await fetch('/_lector/tree');
      if (!response.ok) return;
      html = await response.text();
    } catch {
      return;
    }
    const open = new Set(
      [...sidebar.querySelectorAll('details[open]')].map((d) => d.dataset.path),
    );
    const scroll = sidebar.scrollTop;
    sidebar.innerHTML = html;
    for (const details of sidebar.querySelectorAll('details')) {
      if (open.has(details.dataset.path)) details.open = true;
    }
    sidebar.scrollTop = scroll;
    markCurrent(false);
  }
  window.addEventListener('focus', refreshTree);

  function setSidebarHidden(hidden) {
    body.classList.toggle('sidebar-hidden', hidden);
    prefs.set('sidebarHidden', hidden);
  }

  function setSidebarWidth(width) {
    const clamped = Math.round(Math.min(Math.max(width, 160), 560));
    document.documentElement.style.setProperty('--sidebar-width', `${clamped}px`);
    return clamped;
  }

  resizer.addEventListener('pointerdown', (event) => {
    event.preventDefault();
    resizer.setPointerCapture(event.pointerId);
    body.classList.add('resizing');
    let width = sidebar.offsetWidth;
    const move = (e) => { width = setSidebarWidth(e.clientX); };
    const stop = () => {
      resizer.removeEventListener('pointermove', move);
      body.classList.remove('resizing');
      prefs.set('sidebarWidth', width);
    };
    resizer.addEventListener('pointermove', move);
    resizer.addEventListener('pointerup', stop, { once: true });
    resizer.addEventListener('pointercancel', stop, { once: true });
  });

  // ---------- Zoom ----------

  let zoom = prefs.get('zoom', 1);
  function setZoom(value) {
    zoom = Math.round(Math.min(Math.max(value, 0.5), 3) * 10) / 10;
    document.documentElement.style.zoom = zoom;
    prefs.set('zoom', zoom);
  }

  // ---------- Keys ----------

  const isEditable = (el) => el && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName));

  document.addEventListener('keydown', (event) => {
    const mod = event.ctrlKey || event.metaKey;
    const key = event.key.length === 1 ? event.key.toLowerCase() : event.key;

    if (key === 'F4' || (mod && key === 'b')) {
      if (body.classList.contains('has-sidebar')) {
        setSidebarHidden(!body.classList.contains('sidebar-hidden'));
      }
    } else if ((key === 'Backspace' && !mod && !isEditable(event.target)) || (event.altKey && key === 'ArrowLeft')) {
      history.back();
    } else if (event.altKey && key === 'ArrowRight') {
      history.forward();
    } else if (key === 'F5' || (mod && key === 'r')) {
      location.reload();
    } else if (mod && (key === '=' || key === '+')) {
      setZoom(zoom + 0.1);
    } else if (mod && key === '-') {
      setZoom(zoom - 0.1);
    } else if (mod && key === '0') {
      setZoom(1);
    } else if (mod && (key === 'q' || key === 'w')) {
      window.ipc.postMessage('quit');
    } else {
      return;
    }
    event.preventDefault();
  });

  // ---------- Start ----------

  setZoom(zoom);
  setSidebarWidth(prefs.get('sidebarWidth', 260));
  if (prefs.get('sidebarHidden', false)) body.classList.add('sidebar-hidden');
  markCurrent(true);
  if (location.hash) scrollToHash(location.hash);
  // Restored sidebar state shouldn't animate in; enable transitions after the first paint.
  requestAnimationFrame(() => requestAnimationFrame(() => body.classList.remove('booting')));
})();
