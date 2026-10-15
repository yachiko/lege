// The viewer: in-page navigation, live reload, sidebar, search and keys.
// The page URL is always the open document's path; see src/server.rs.
(() => {
  'use strict';

  const MARKDOWN = /\.(md|markdown|mdown|mkd)$/i;
  const POLL_MS = 500;

  const body = document.body;
  const page = document.getElementById('page');
  const sidebar = document.getElementById('sidebar');
  const canvas = document.getElementById('canvas');
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
  // True while a sharded document is still arriving.
  let loading = false;

  const decode = (path) => {
    try { return decodeURIComponent(path); } catch { return path; }
  };
  const samePath = (a, b) => decode(a) === decode(b);
  const hasDocument = () => MARKDOWN.test(decode(location.pathname));

  // ---------- Documents ----------

  async function show(url, { push = false, scroll = null, find: findTarget = null } = {}) {
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
    nearScreen.disconnect();
    find = null;
    page.innerHTML = html;
    setFind(findTarget);
    shownPath = url.pathname;
    const name = decode(url.pathname).split('/').pop();
    document.title = `${name} — Lege`;

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
    loading = total > 1;
    for (let i = 1; i < total; i++) {
      const html = await next;
      if (token !== loadToken) return;
      if (i + 1 < total) next = fetchShard(i + 1);
      if (html) {
        page.insertAdjacentHTML('beforeend', html);
        extendOutline(page.lastElementChild);
        addShard(page.lastElementChild);
        settleScroll();
      }
      await new Promise((resolve) => setTimeout(resolve, 0));
    }
    if (token !== loadToken) return;
    loading = false;
    if (pendingScroll !== null) {
      window.scrollTo(0, pendingScroll);
      pendingScroll = null;
    }
    settleFind(true);
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
    settleFind(!loading);
  }

  // Scrolling by hand while a document loads cancels any pending jump.
  // (Input in the search box is not scrolling.)
  for (const type of ['wheel', 'touchmove', 'keydown', 'mousedown']) {
    window.addEventListener(type, (event) => {
      if (event.target?.closest?.('#search')) return;
      pendingScroll = null;
      pendingHash = '';
      if (find) find.jump = false;
    }, { passive: true });
  }

  // ---------- Find on page ----------
  // After a search result opens, matches on the page are highlighted and the
  // chosen one scrolled to; F3 / Shift+F3 step through them. Matches are
  // counted per shard up front, but marked up only in shards near the screen,
  // so documents with tens of thousands of matches stay responsive. Matching
  // runs on the rendered text, so a match can span formatting like **bold**.

  // { query, index, jump, current, currentMark, parts: [{ shard, count, marks }] }
  let find = null;

  const nearScreen = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      const part = find?.parts.find((p) => p.shard === entry.target);
      if (part) markPart(part);
    }
  }, { rootMargin: '100% 0px' });

  function setFind(target) {
    clearFind();
    if (!target) return;
    find = {
      query: target.query,
      index: target.index ?? 0,
      jump: target.jump ?? true,
      current: -1,
      currentMark: null,
      parts: [],
    };
    for (const shard of page.querySelectorAll('.shard')) addShard(shard);
  }

  // Counts the matches in a newly shown shard; marking up waits until it's near.
  function addShard(shard) {
    if (!find || !shard) return;
    find.parts.push({ shard, count: matchOffsets(shard.textContent).length, marks: null });
    nearScreen.observe(shard);
  }

  function clearFind() {
    nearScreen.disconnect();
    const parents = new Set();
    for (const mark of page.querySelectorAll('mark.hit')) {
      parents.add(mark.parentNode);
      mark.replaceWith(...mark.childNodes);
    }
    for (const parent of parents) parent.normalize();
    find = null;
  }

  // Where the query occurs in `text`, ignoring case when that keeps offsets intact.
  function matchOffsets(text) {
    let haystack = text.toLowerCase();
    let needle = find.query.toLowerCase();
    if (haystack.length !== text.length || needle.length !== find.query.length) {
      haystack = text;
      needle = find.query;
    }
    const found = [];
    if (!needle) return found;
    for (let i = haystack.indexOf(needle); i !== -1; i = haystack.indexOf(needle, i + needle.length)) {
      found.push(i);
    }
    return found;
  }

  function markPart(part) {
    if (part.marks) return;
    const nodes = [];
    const starts = [];
    let text = '';
    const walker = document.createTreeWalker(part.shard, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      nodes.push(node);
      starts.push(text.length);
      text += node.data;
    }
    const found = matchOffsets(text);
    const length = find.query.length;

    // Wrap from the last match back: splitting a text node keeps its head in
    // place, so the offsets of earlier matches stay valid.
    part.marks = new Array(found.length);
    let k = nodes.length - 1;
    for (let f = found.length - 1; f >= 0; f--) {
      const start = found[f];
      const end = start + length;
      while (k > 0 && starts[k] >= end) k--;
      for (let j = k; j >= 0 && starts[j] + nodes[j].data.length > start; j--) {
        const from = Math.max(start - starts[j], 0);
        const to = Math.min(end - starts[j], nodes[j].data.length);
        if (from < to) part.marks[f] = wrapText(nodes[j], from, to);
      }
    }
  }

  function wrapText(node, from, to) {
    let middle = node;
    if (from > 0) middle = node.splitText(from);
    if (to - from < middle.data.length) middle.splitText(to - from);
    const mark = document.createElement('mark');
    mark.className = 'hit';
    middle.replaceWith(mark);
    mark.append(middle);
    return mark;
  }

  const totalHits = () => find.parts.reduce((sum, part) => sum + part.count, 0);

  // Jumps to the chosen match once its shard has arrived. The index counts
  // matches in the Markdown source, which can differ slightly from the
  // rendered text (e.g. matches in link URLs), so `final` settles for the last.
  function settleFind(final) {
    if (!find?.jump) return;
    const total = totalHits();
    if (total > find.index) {
      goToHit(find.index);
      find.jump = false;
    } else if (final && total > 0) {
      goToHit(total - 1);
      find.jump = false;
    }
  }

  function goToHit(index) {
    const total = find ? totalHits() : 0;
    if (total === 0) return;
    index = (index + total) % total;
    let offset = 0;
    let part = find.parts[0];
    for (part of find.parts) {
      if (index < offset + part.count) break;
      offset += part.count;
    }
    markPart(part);
    find.currentMark?.classList.remove('current');
    find.current = index;
    find.currentMark = part.marks[index - offset] ?? null;
    find.currentMark?.classList.add('current');
    find.currentMark?.scrollIntoView({ block: 'center' });
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
      // Keep the highlights, but stay where the reader is.
      const keep = find && { query: find.query, index: find.current, jump: false };
      show(new URL(location.href), { scroll: window.scrollY, find: keep });
    }
  }, POLL_MS);

  document.addEventListener('click', (event) => {
    if (event.defaultPrevented || event.button !== 0) return;
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const link = event.target.closest('a[href]');
    if (!link) return;
    const url = new URL(link.href, location.href);

    // Other schemes and non-Markdown files fall through to the native
    // navigation, which Lege hands to the system's default app.
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
      const response = await fetch('/_lege/tree');
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

  document.getElementById('sidebar-toggle').addEventListener('click', () => {
    setSidebarHidden(!body.classList.contains('sidebar-hidden'));
  });

  // Dragging the edge resizes; letting go near the window edge collapses the
  // sidebar instead, keeping the width it had before the drag.
  resizer.addEventListener('pointerdown', (event) => {
    event.preventDefault();
    resizer.setPointerCapture(event.pointerId);
    body.classList.add('resizing');
    const before = sidebar.offsetWidth;
    let width = before;
    let x = event.clientX;
    const move = (e) => {
      x = e.clientX;
      width = setSidebarWidth(x);
    };
    const stop = () => {
      resizer.removeEventListener('pointermove', move);
      body.classList.remove('resizing');
      if (x < 100) {
        setSidebarWidth(before);
        setSidebarHidden(true);
      } else {
        prefs.set('sidebarWidth', width);
      }
    };
    resizer.addEventListener('pointermove', move);
    resizer.addEventListener('pointerup', stop, { once: true });
    resizer.addEventListener('pointercancel', stop, { once: true });
  });

  // ---------- Page width ----------
  // Dragging either edge of the page widens or narrows it around its centre,
  // up to the room the window has; double-clicking an edge goes back to the
  // paper's width. The text under the top of the window stays in place.

  const PAGE_WIDTH = 816;

  function setPageWidth(width) {
    const clamped = Math.round(Math.max(width, 480));
    document.documentElement.style.setProperty('--page-width', `${clamped}px`);
    return clamped;
  }

  // The block at the reading line, and where it is, so a reflow can put it back.
  function readingAnchor() {
    const rect = page.getBoundingClientRect();
    let el = document.elementFromPoint(rect.left + rect.width / 2, 96);
    if (!el || el === page || !page.contains(el)) return null;
    while (el.parentElement !== page && !el.parentElement.classList.contains('shard')) el = el.parentElement;
    return { el, top: el.getBoundingClientRect().top };
  }

  function restoreAnchor(anchor) {
    if (anchor) window.scrollBy(0, anchor.el.getBoundingClientRect().top - anchor.top);
  }

  for (const edge of document.querySelectorAll('.page-edge')) {
    const side = edge.classList.contains('left') ? -1 : 1;

    edge.addEventListener('pointerdown', (event) => {
      if (event.button !== 0) return;
      event.preventDefault();
      edge.setPointerCapture(event.pointerId);
      body.classList.add('resizing');
      edge.classList.add('dragging');
      const start = page.offsetWidth;
      // Pointer positions are in zoomed pixels; widths are in CSS pixels.
      const scale = page.getBoundingClientRect().width / start;
      const padding = getComputedStyle(canvas);
      const room = canvas.clientWidth - parseFloat(padding.paddingLeft) - parseFloat(padding.paddingRight);
      const anchor = readingAnchor();
      let width = start;
      const move = (e) => {
        // Both edges move, so the page grows by twice the pointer's travel.
        width = setPageWidth(Math.min(start + (2 * side * (e.clientX - event.clientX)) / scale, room));
        restoreAnchor(anchor);
      };
      const stop = () => {
        edge.removeEventListener('pointermove', move);
        body.classList.remove('resizing');
        edge.classList.remove('dragging');
        prefs.set('pageWidth', width);
      };
      edge.addEventListener('pointermove', move);
      edge.addEventListener('pointerup', stop, { once: true });
      edge.addEventListener('pointercancel', stop, { once: true });
    });

    edge.addEventListener('dblclick', () => {
      const anchor = readingAnchor();
      prefs.set('pageWidth', setPageWidth(PAGE_WIDTH));
      restoreAnchor(anchor);
    });
  }

  // ---------- Zoom ----------

  let zoom = prefs.get('zoom', 1);
  function setZoom(value) {
    zoom = Math.round(Math.min(Math.max(value, 0.5), 3) * 10) / 10;
    document.documentElement.style.zoom = zoom;
    prefs.set('zoom', zoom);
  }

  // ---------- Search ----------
  // Ctrl/Cmd+K or / opens a search box over the page: file names (fuzzy) and
  // text across the browsed folder, or just the open file in file mode.

  const search = document.getElementById('search');
  const searchInput = document.getElementById('search-input');
  const searchResults = document.getElementById('search-results');
  let searchSeq = 0;
  let searchTimer = 0;
  let selected = -1;
  // The query the visible results are for, and whether Enter is waiting on
  // results for a newer one.
  let shownQuery = '';
  let enterPending = false;

  function openSearch() {
    body.classList.add('searching');
    search.hidden = false;
    searchInput.focus();
    searchInput.select();
  }

  function closeSearch() {
    enterPending = false;
    body.classList.remove('searching');
    search.hidden = true;
    searchInput.blur();
  }

  async function runSearch() {
    const query = searchInput.value.trim();
    const seq = ++searchSeq;
    let html = '';
    if (query) {
      const doc = hasDocument() ? decode(location.pathname) : '';
      try {
        const params = `q=${encodeURIComponent(query)}&doc=${encodeURIComponent(doc)}`;
        const response = await fetch(`/_lege/search?${params}`);
        if (response.ok) html = await response.text();
      } catch {}
    }
    if (seq !== searchSeq) return; // a newer search has started
    searchResults.innerHTML = html;
    searchResults.scrollTop = 0;
    shownQuery = query;
    selected = -1;
    select(0);
    if (enterPending) {
      enterPending = false;
      openResult(resultLinks()[0]);
    }
  }

  // Enter opens the selected result, but if the results on screen are for
  // an older query (typing was faster than the search), it waits for the
  // current one instead of opening a stale match.
  function enter() {
    if (searchInput.value.trim() === shownQuery) {
      openResult(resultLinks()[selected]);
      return;
    }
    enterPending = true;
    clearTimeout(searchTimer);
    runSearch();
  }

  const resultLinks = () => [...searchResults.querySelectorAll('a.result')];

  function select(index, reveal = true) {
    const links = resultLinks();
    links[selected]?.classList.remove('selected');
    if (links.length === 0) {
      selected = -1;
      return;
    }
    selected = (index + links.length) % links.length;
    const link = links[selected];
    link.classList.add('selected');
    if (!reveal) return;
    // Both are positioned within #search, so this is the offset in the list.
    const top = link.offsetTop - searchResults.offsetTop;
    if (top < searchResults.scrollTop) {
      searchResults.scrollTop = top;
    } else if (top + link.offsetHeight > searchResults.scrollTop + searchResults.clientHeight) {
      searchResults.scrollTop = top + link.offsetHeight - searchResults.clientHeight;
    }
  }

  function openResult(link) {
    if (!link) return;
    closeSearch();
    const url = new URL(link.href);
    const target = link.dataset.find
      ? { query: link.dataset.find, index: Number(link.dataset.index) }
      : null;
    if (samePath(url.pathname, location.pathname)) {
      clearFind();
      setFind(target);
      settleFind(!loading);
    } else {
      show(url, { push: true, find: target });
    }
  }

  searchInput.addEventListener('input', () => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(runSearch, 80);
  });

  searchInput.addEventListener('keydown', (event) => {
    if (event.key === 'ArrowDown') select(selected + 1);
    else if (event.key === 'ArrowUp') select(selected - 1);
    else if (event.key === 'Enter') enter();
    else if (event.key === 'Escape') closeSearch();
    else return;
    event.preventDefault();
    event.stopPropagation();
  });

  searchResults.addEventListener('click', (event) => {
    const link = event.target.closest('a.result');
    if (!link) return;
    event.preventDefault();
    openResult(link);
  });

  searchResults.addEventListener('mousemove', (event) => {
    const link = event.target.closest('a.result');
    const index = link ? resultLinks().indexOf(link) : -1;
    if (index !== -1 && index !== selected) select(index, false);
  });

  document.addEventListener('mousedown', (event) => {
    if (!search.hidden && !search.contains(event.target)) closeSearch();
  });

  // ---------- Keys ----------

  const isEditable = (el) => el && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName));

  document.addEventListener('keydown', (event) => {
    const mod = event.ctrlKey || event.metaKey;
    const key = event.key.length === 1 ? event.key.toLowerCase() : event.key;

    if ((mod && key === 'k') || (key === '/' && !isEditable(event.target))) {
      openSearch();
    } else if (key === 'F3' || (mod && key === 'g')) {
      if (find) goToHit(find.current + (event.shiftKey ? -1 : 1));
    } else if (key === 'Escape' && find && !isEditable(event.target)) {
      clearFind();
    } else if (key === 'F4' || (mod && key === 'b')) {
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
  setPageWidth(prefs.get('pageWidth', PAGE_WIDTH));
  if (prefs.get('sidebarHidden', false)) body.classList.add('sidebar-hidden');
  markCurrent(true);
  pendingHash = location.hash;
  settleScroll();
  loadShards(location.pathname, Number(body.dataset.shards) || 1, ++loadToken);
  // Restored sidebar state shouldn't animate in; enable transitions after the first paint.
  requestAnimationFrame(() => requestAnimationFrame(() => body.classList.remove('booting')));
})();
