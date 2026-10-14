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
  // Bumped on every document load, so a superseded load stops adding shards.
  let loadToken = 0;
  // Where to scroll once enough of a progressively loading document is in.
  let pendingScroll = null;
  let pendingHash = '';

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
    const shards = Number(response.headers.get('X-Shards')) || 1;

    if (push) {
      history.replaceState({ scroll: window.scrollY }, '');
      history.pushState({}, '', url.pathname + url.hash);
    }
    const token = ++loadToken;
    page.innerHTML = html;
    shownPath = url.pathname;
    const name = decode(url.pathname).split('/').pop();
    document.title = `${name} — lector`;

    pendingScroll = scroll;
    pendingHash = scroll === null ? url.hash : '';
    if (scroll === null && !url.hash) window.scrollTo(0, 0);
    markCurrent(true);
    settleScroll();
    loadShards(url.pathname, shards, token);
    mtime = await fetchMtime();
  }

  // Large documents arrive in shards: the first comes with the page, the
  // rest are appended one per task, so the page paints and stays responsive
  // as it grows. The next shard is fetched while the current one goes in.
  async function loadShards(path, total, token) {
    const fetchShard = (i) =>
      fetch(`${path}?shard=${i}`).then((r) => (r.ok ? r.text() : '')).catch(() => '');
    let next = total > 1 ? fetchShard(1) : null;
    for (let i = 1; i < total; i++) {
      const html = await next;
      if (token !== loadToken) return;
      if (i + 1 < total) next = fetchShard(i + 1);
      if (html) {
        page.insertAdjacentHTML('beforeend', html);
        extendOutline(page.lastElementChild);
        settleScroll();
      }
      await new Promise((resolve) => setTimeout(resolve, 0));
    }
    if (token === loadToken && pendingScroll !== null) {
      window.scrollTo(0, pendingScroll);
      pendingScroll = null;
    }
  }

  // Scrolls to the pending position or #anchor once its content has arrived.
  function settleScroll() {
    const height = document.documentElement.scrollHeight;
    if (pendingScroll !== null && height >= pendingScroll + window.innerHeight) {
      window.scrollTo(0, pendingScroll);
      pendingScroll = null;
    }
    if (pendingHash && document.getElementById(decode(pendingHash.slice(1)))) {
      scrollToHash(pendingHash);
      pendingHash = '';
    }
  }

  // Scrolling by hand while a document loads cancels any pending jump.
  for (const type of ['wheel', 'touchmove', 'keydown', 'mousedown']) {
    window.addEventListener(type, () => {
      pendingScroll = null;
      pendingHash = '';
    }, { passive: true });
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
      // The target may be in a shard that hasn't arrived yet.
      pendingHash = location.hash;
      settleScroll();
      return;
    }
    show(new URL(location.href), { scroll: event.state?.scroll ?? null });
  });

  // ---------- Sidebar ----------

  function markCurrent(reveal) {
    let current = null;
    for (const link of sidebar.querySelectorAll('a[href]:not(.heading)')) {
      const isCurrent = samePath(new URL(link.href).pathname, location.pathname);
      link.toggleAttribute('aria-current', isCurrent);
      link.parentElement.classList.toggle('current', isCurrent);
      if (isCurrent) current = link;
    }
    buildOutline(current);
    if (!current || !reveal) return;
    for (let el = current.parentElement; el && el !== sidebar; el = el.parentElement) {
      if (el.tagName === 'DETAILS') el.open = true;
    }
    revealInSidebar(current, 1 / 3);
  }

  // Scrolls only the sidebar (scrollIntoView would also move the document),
  // placing `el` at `position` of its height if it is out of view.
  function revealInSidebar(el, position) {
    const header = sidebar.querySelector('header')?.offsetHeight ?? 0;
    const top = el.offsetTop; // relative to the sidebar, its offsetParent
    const visibleTop = sidebar.scrollTop + header;
    const visibleBottom = sidebar.scrollTop + sidebar.clientHeight;
    if (top < visibleTop || top + el.offsetHeight > visibleBottom) {
      sidebar.scrollTop = top - header - (sidebar.clientHeight - header) * position;
    }
  }

  // ---------- Outline ----------
  // The open document's sections, listed under its file in the sidebar,
  // with the one being read highlighted.

  let outline = null;
  let headings = [];
  let outlineLinks = [];
  let activeIndex = -1;

  function buildOutline(current) {
    sidebar.querySelector('.outline')?.remove();
    outline = null;
    headings = [];
    outlineLinks = [];
    activeIndex = -1;
    if (!current) return;

    outline = document.createElement('ul');
    outline.className = 'outline';
    current.after(outline);
    extendOutline(page);
  }

  // Adds the sections in `container` (the page, or a newly arrived shard).
  function extendOutline(container) {
    if (!outline || !container) return;
    for (const heading of container.querySelectorAll('h2[id]')) {
      const link = document.createElement('a');
      link.className = 'heading';
      link.href = `#${encodeURIComponent(heading.id)}`;
      link.textContent = link.title = heading.textContent.trim();
      const item = document.createElement('li');
      item.append(link);
      outline.append(item);
      headings.push(heading);
      outlineLinks.push(link);
    }
    updateActive();
  }

  // The active section is the last heading above a line near the top of the
  // window. Headings are in document order, so binary search keeps this cheap
  // even for documents with thousands of sections.
  function updateActive() {
    if (headings.length === 0) return;
    const line = 96;
    let lo = 0;
    let hi = headings.length - 1;
    let found = -1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (headings[mid].getBoundingClientRect().top <= line) {
        found = mid;
        lo = mid + 1;
      } else {
        hi = mid - 1;
      }
    }
    // Short last sections never reach the line; scrolled to the end means the last one.
    const atEnd = window.scrollY > 0 && window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - 2;
    if (atEnd) found = headings.length - 1;
    if (found === activeIndex) return;

    outlineLinks[activeIndex]?.classList.remove('active');
    activeIndex = found;
    const link = outlineLinks[found];
    if (!link) return;
    link.classList.add('active');
    if (!body.classList.contains('sidebar-hidden')) revealInSidebar(link, 1 / 2);
  }

  let spyQueued = false;
  window.addEventListener('scroll', () => {
    if (spyQueued) return;
    spyQueued = true;
    requestAnimationFrame(() => {
      spyQueued = false;
      updateActive();
    });
  }, { passive: true });

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
  pendingHash = location.hash;
  settleScroll();
  loadShards(location.pathname, Number(body.dataset.shards) || 1, ++loadToken);
  // Restored sidebar state shouldn't animate in; enable transitions after the first paint.
  requestAnimationFrame(() => requestAnimationFrame(() => body.classList.remove('booting')));
})();
