// StockWise web dashboard: plain JavaScript, no dependencies, no build step.
//
// Rules this file keeps (src/server/web.rs tests check them):
// - API data reaches the page only as text (text nodes, textContent or
//   attribute values set with setAttribute), never parsed as markup.
// - No inline event handlers; every listener is attached here.
// - The API token is kept in sessionStorage and sent only in the
//   Authorization header of same-origin requests, never in a URL.
// - Read-only: the page only sends GET requests.
(() => {
  'use strict';

  const REFRESH_MS = 30000;
  const TOKEN_KEY = 'stockwise.token';
  const PERIODS = [
    ['1d', '1D', '1 day'],
    ['5d', '5D', '5 days'],
    ['1mo', '1M', '1 month'],
    ['3mo', '3M', '3 months'],
    ['6mo', '6M', '6 months'],
    ['1y', '1Y', '1 year'],
    ['5y', '5Y', '5 years'],
    ['max', 'Max', 'all time'],
  ];
  const DEFAULT_PERIOD = '1mo';
  // Same rule as the server's check_symbol.
  const SYMBOL_RE = /^[A-Za-z0-9.^=&_-]{1,32}$/;
  // Allocation colors (--s1 .. --s7); smaller holdings fold into "Other".
  const ALLOC_SLOTS = 7;
  const SVG_NS = 'http://www.w3.org/2000/svg';
  const DASH = '—';
  const MINUS = '−';

  const $ = (id) => document.getElementById(id);

  // ── DOM helpers: build nodes, never parse markup ──

  function setAttrs(node, props) {
    for (const [k, v] of Object.entries(props || {})) {
      if (v == null || v === false) continue;
      if (/^on/i.test(k)) throw new Error('inline event handlers are not allowed');
      if (k === 'class') node.setAttribute('class', v);
      else if (k === 'text') node.textContent = v;
      else node.setAttribute(k, v === true ? '' : String(v));
    }
  }

  function el(tag, props, children) {
    const node = document.createElement(tag);
    setAttrs(node, props);
    for (const c of children || []) if (c != null) node.append(c);
    return node;
  }

  function svgEl(tag, props, children) {
    const node = document.createElementNS(SVG_NS, tag);
    setAttrs(node, props);
    for (const c of children || []) if (c != null) node.append(c);
    return node;
  }

  const code = (text) => el('code', { text });

  // ── Numbers ──

  const isNum = (v) => typeof v === 'number' && Number.isFinite(v);
  const CURRENCY_SIGN = { INR: '₹', USD: '$', EUR: '€', GBP: '£', JPY: '¥' };
  const formats = new Map();

  // Indian digit grouping (12,34,567.80) for INR, Western otherwise.
  function grouping(currency, digits) {
    const locale = currency === 'INR' ? 'en-IN' : 'en-US';
    const key = locale + digits;
    if (!formats.has(key)) {
      formats.set(key, new Intl.NumberFormat(locale, {
        minimumFractionDigits: digits,
        maximumFractionDigits: digits,
      }));
    }
    return formats.get(key);
  }

  function fmtNumber(v, currency, digits = 2) {
    if (!isNum(v)) return DASH;
    const s = grouping(currency, digits).format(Math.abs(v));
    return v < 0 ? MINUS + s : s;
  }

  function withSign(v, currency, body) {
    const sign = v < 0 ? MINUS : '';
    const sym = CURRENCY_SIGN[currency];
    if (sym) return sign + sym + body;
    return sign + body + (currency ? ' ' + currency : '');
  }

  function fmtMoney(v, currency, digits = 2) {
    if (!isNum(v)) return DASH;
    return withSign(v, currency, grouping(currency, digits).format(Math.abs(v)));
  }

  // Headline figures: lakhs and crores for INR (₹48.25 L, ₹1.20 Cr), as the
  // CLI prints them; K/M/B for other currencies.
  function fmtCompact(v, currency) {
    if (!isNum(v)) return DASH;
    const units = currency === 'INR'
      ? [[1e12, 'L Cr'], [1e10, 'K Cr'], [1e7, 'Cr'], [1e5, 'L']]
      : [[1e12, 'T'], [1e9, 'B'], [1e6, 'M']];
    const a = Math.abs(v);
    for (const [size, unit] of units) {
      if (a >= size) return withSign(v, currency, (a / size).toFixed(2) + ' ' + unit);
    }
    return fmtMoney(v, currency);
  }

  function fmtCount(v, indian) {
    if (!isNum(v)) return DASH;
    const units = indian ? [[1e7, ' Cr'], [1e5, ' L']] : [[1e9, 'B'], [1e6, 'M'], [1e3, 'K']];
    for (const [size, unit] of units) {
      if (v >= size) return (v / size).toFixed(2) + unit;
    }
    return grouping(indian ? 'INR' : '', 0).format(v);
  }

  function fmtQty(v) {
    if (!isNum(v)) return DASH;
    return new Intl.NumberFormat('en-IN', { maximumFractionDigits: 4 }).format(v);
  }

  function fmtPct(v, digits = 2) {
    if (!isNum(v)) return DASH;
    const sign = v > 0 ? '+' : v < 0 ? MINUS : '';
    return sign + Math.abs(v).toFixed(digits) + '%';
  }

  function fmtSignedMoney(v, currency) {
    if (!isNum(v)) return DASH;
    return (v > 0 ? '+' : '') + fmtMoney(v, currency);
  }

  const direction = (v) => (isNum(v) && v !== 0 ? (v > 0 ? 'up' : 'down') : '');

  // "▲ +1.20%": the arrow repeats the sign for people who can't tell the
  // colors apart; screen readers skip it.
  function change(pct, extra) {
    const dir = direction(pct);
    return el('span', { class: dir }, [
      dir ? el('span', { 'aria-hidden': 'true', text: dir === 'up' ? '▲ ' : '▼ ' }) : null,
      fmtPct(pct),
      extra ? ' (' + extra + ')' : null,
    ]);
  }

  const timeFmt = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' });

  // ── API ──

  class AuthError extends Error {}
  class HttpError extends Error {
    constructor(status, message) {
      super(message);
      this.status = status;
    }
  }

  let memoryToken = null;

  function getToken() {
    try {
      return sessionStorage.getItem(TOKEN_KEY) || memoryToken;
    } catch {
      return memoryToken;
    }
  }

  function setToken(token) {
    memoryToken = token;
    try {
      if (token) sessionStorage.setItem(TOKEN_KEY, token);
      else sessionStorage.removeItem(TOKEN_KEY);
    } catch {
      // Storage disabled: the token lives in memory until the tab closes.
    }
    $('signout').hidden = !token;
  }

  // GET a same-origin JSON endpoint. `path` always starts with /api/.
  async function api(path) {
    const headers = { Accept: 'application/json' };
    const token = getToken();
    if (token) headers.Authorization = 'Bearer ' + token;
    let resp;
    try {
      resp = await fetch(path, { headers, credentials: 'omit', cache: 'no-store', redirect: 'error' });
    } catch {
      throw new HttpError(0, 'The StockWise server is not reachable');
    }
    if (resp.status === 401) throw new AuthError(token ? 'That token was not accepted.' : '');
    let body = null;
    try {
      body = await resp.json();
    } catch {
      // Not JSON; the status code says enough.
    }
    if (!resp.ok) {
      const msg = body && typeof body.error === 'string' ? body.error : 'HTTP ' + resp.status;
      throw new HttpError(resp.status, msg);
    }
    return body;
  }

  const query = (params) => new URLSearchParams(params).toString();

  // Symbols on this page are full Yahoo symbols (TCS.NS, ^NSEI, AAPL,
  // BTC-USD). `market=us` passes them through unchanged instead of adding the
  // server's default suffix to bare tickers.
  const EXACT = { market: 'us' };

  // ── State ──

  const state = {
    symbol: null,
    period: DEFAULT_PERIOD,
    base: '',
    data: { indices: null, watchlist: null, portfolio: null, alerts: null },
    errors: { indices: null, watchlist: null, portfolio: null, alerts: null },
    sort: { key: null, dir: -1 },
    hist: null,
    quote: null,
    chartError: null,
    chartSeq: 0,
    timer: null,
    refreshing: false,
    needsToken: false,
    lastOk: 0,
  };

  function readHash() {
    const p = new URLSearchParams(location.hash.slice(1));
    const symbol = p.get('symbol');
    const period = p.get('period');
    if (symbol && SYMBOL_RE.test(symbol)) state.symbol = symbol.toUpperCase();
    if (period && PERIODS.some(([k]) => k === period)) state.period = period;
  }

  function writeHash() {
    const p = new URLSearchParams();
    if (state.symbol) p.set('symbol', state.symbol);
    p.set('period', state.period);
    history.replaceState(null, '', '#' + p.toString());
  }

  // ── Banner and login ──

  function showBanner(text, signIn) {
    $('banner-text').textContent = text;
    $('banner-signin').hidden = !signIn;
    $('banner').hidden = false;
  }

  function hideBanner() {
    $('banner').hidden = true;
  }

  function openLogin(message) {
    state.needsToken = true;
    clearTimeout(state.timer);
    showBanner('This server needs its API token to show your data.', true);
    const dialog = $('login');
    $('login-error').textContent = message || '';
    $('token').value = '';
    if (!dialog.open) dialog.showModal();
    $('token').focus();
  }

  function onLogin(ev) {
    ev.preventDefault();
    const token = $('token').value.trim();
    // Header values must be visible ASCII.
    if (!/^[\x21-\x7e]{1,512}$/.test(token)) {
      $('login-error').textContent = 'A token is printable ASCII without spaces.';
      return;
    }
    $('token').value = '';
    setToken(token);
    $('login').close();
    state.needsToken = false;
    hideBanner();
    refreshAll(true);
  }

  function signOut() {
    setToken(null);
    for (const k of Object.keys(state.data)) state.data[k] = null;
    state.hist = null;
    state.quote = null;
    renderAll();
    renderQuote();
    renderChart();
    openLogin('');
  }

  // ── Loading ──

  async function loadSection(name, path) {
    try {
      state.data[name] = await api(path);
      state.errors[name] = null;
      return true;
    } catch (e) {
      if (e instanceof AuthError) throw e;
      state.errors[name] = e;
      return false;
    }
  }

  function portfolioPath() {
    return '/api/portfolio' + (state.base ? '?' + query({ base: state.base }) : '');
  }

  async function refreshAll(withChart) {
    if (state.refreshing || state.needsToken) return;
    state.refreshing = true;
    clearTimeout(state.timer);
    $('refresh').setAttribute('aria-busy', 'true');
    const results = await Promise.allSettled([
      loadSection('indices', '/api/markets'),
      loadSection('watchlist', '/api/watchlist'),
      loadSection('portfolio', portfolioPath()),
      loadSection('alerts', '/api/alerts'),
    ]);
    state.refreshing = false;
    $('refresh').removeAttribute('aria-busy');

    const auth = results.find((r) => r.status === 'rejected' && r.reason instanceof AuthError);
    if (auth) {
      if (getToken()) setToken(null);
      openLogin(auth.reason.message);
      return;
    }
    if (!state.symbol) pickDefaultSymbol();
    renderAll();

    const ok = results.filter((r) => r.status === 'fulfilled' && r.value).length;
    const unreachable = Object.values(state.errors).some((e) => e && e.status === 0);
    if (ok > 0) {
      state.lastOk = Date.now();
      $('updated').textContent = 'Updated ' + timeFmt.format(new Date()) + ' · every 30 s';
    }
    if (unreachable) showBanner('Cannot reach the StockWise server. Retrying every 30 seconds.', false);
    else if (!state.needsToken) hideBanner();

    // Intraday charts move; longer ones are reloaded only when asked.
    const intraday = state.period === '1d' || state.period === '5d';
    await loadChart(withChart || intraday || !state.hist);
    schedule();
  }

  function schedule() {
    clearTimeout(state.timer);
    if (!document.hidden && !state.needsToken) state.timer = setTimeout(() => refreshAll(false), REFRESH_MS);
  }

  function onVisibility() {
    if (document.hidden) {
      clearTimeout(state.timer);
    } else if (!state.needsToken) {
      if (Date.now() - state.lastOk >= REFRESH_MS) refreshAll(false);
      else schedule();
    }
  }

  // First watchlist symbol, else the largest holding, else the first index.
  function pickDefaultSymbol() {
    const wl = state.data.watchlist || [];
    const holdings = ((state.data.portfolio || {}).holdings || []).slice()
      .sort((a, b) => (b.weight_percent || 0) - (a.weight_percent || 0));
    const indices = state.data.indices || [];
    const first = [...wl, ...holdings, ...indices].map((r) => r.symbol).find((s) => s && SYMBOL_RE.test(s));
    if (first) {
      state.symbol = first;
      writeHash();
    }
  }

  function selectSymbol(symbol, scroll) {
    if (!symbol || !SYMBOL_RE.test(symbol)) return;
    state.symbol = symbol;
    state.hist = null;
    state.quote = null;
    writeHash();
    renderAll();
    loadChart(true);
    if (scroll) {
      const card = $('chart-card');
      const r = card.getBoundingClientRect();
      if (r.top < 0 || r.top > window.innerHeight * 0.5) {
        const smooth = window.matchMedia('(prefers-reduced-motion: no-preference)').matches;
        card.scrollIntoView({ behavior: smooth ? 'smooth' : 'auto', block: 'start' });
      }
    }
  }

  // Delegated click handler for every [data-symbol] button.
  function onSymbolClick(ev) {
    const btn = ev.target.closest('button[data-symbol]');
    if (btn) selectSymbol(btn.getAttribute('data-symbol'), true);
  }

  // ── Sections ──

  function renderAll() {
    renderIndices();
    renderWatchlist();
    renderPortfolio();
    renderAlerts();
  }

  // Previous data stays on screen, dimmed, with the error above it.
  function frame(box, name, build, emptyText) {
    const data = state.data[name];
    const err = state.errors[name];
    box.removeAttribute('aria-busy');
    const parts = [];
    if (err) parts.push(el('p', { class: 'error small', text: 'Could not refresh: ' + err.message }));
    if (data == null) {
      if (!err) parts.push(el('p', { class: 'empty', text: emptyText || 'Loading…' }));
      box.replaceChildren(...parts);
      return;
    }
    const body = build(data);
    if (err) body.classList.add('stale');
    parts.push(body);
    box.replaceChildren(...parts);
  }

  const SHORT_REGION = { 'Indian Markets': 'India', 'US Markets': 'US' };

  function renderIndices() {
    const box = $('indices');
    const rows = state.data.indices;
    box.removeAttribute('aria-busy');
    if (!rows) {
      const err = state.errors.indices;
      box.replaceChildren(el('p', { class: err ? 'error small' : 'empty', text: err ? 'Indices: ' + err.message : 'Loading indices…' }));
      return;
    }
    const groups = [];
    let group = null;
    for (const r of rows) {
      if (!group || group.region !== r.region) {
        group = { region: r.region, node: el('div', { class: 'group', role: 'group', 'aria-label': r.region }) };
        group.node.append(el('span', { class: 'region', 'aria-hidden': 'true', text: SHORT_REGION[r.region] || r.region }));
        groups.push(group);
      }
      if (r.symbol === 'IN10Y') {
        // The India 10-year G-sec yield: a percentage with no chart behind it.
        group.node.append(el('div', { class: 'tile' }, [
          el('span', { class: 'name', text: r.name }),
          el('span', { class: 'price num', text: isNum(r.price) ? r.price.toFixed(2) + '%' : DASH }),
        ]));
        continue;
      }
      group.node.append(el('button', {
        type: 'button',
        class: 'tile',
        'data-symbol': r.symbol,
        'aria-pressed': String(r.symbol === state.symbol),
      }, [
        el('span', { class: 'name', text: r.name }),
        el('span', { class: 'price num', text: fmtNumber(r.price, r.currency) }),
        el('span', { class: 'small' }, [change(r.change_percent)]),
      ]));
    }
    if (state.errors.indices) groups.forEach((g) => g.node.classList.add('stale'));
    box.replaceChildren(...groups.map((g) => g.node));
  }

  function symbolButton(symbol) {
    return el('button', { type: 'button', class: 'link', 'data-symbol': symbol, text: symbol });
  }

  // Mutual funds (MF:<code>) have no chart: show the scheme name instead.
  function holdingLabel(h) {
    if (SYMBOL_RE.test(h.symbol)) return symbolButton(h.symbol);
    return el('span', { title: h.symbol, text: h.name || h.symbol });
  }

  function renderWatchlist() {
    frame($('watchlist'), 'watchlist', (rows) => {
      if (rows.length === 0) {
        return el('p', { class: 'empty' }, [
          'Your watchlist is empty. Add symbols with ', code('stockwise watch add RELIANCE TCS'), '.',
        ]);
      }
      const body = rows.map((r) => el('tr', { class: r.symbol === state.symbol ? 'selected' : null }, [
        el('th', { scope: 'row' }, [symbolButton(r.symbol)]),
        el('td', { class: 'num', text: fmtMoney(r.price, r.currency) }),
        el('td', { class: 'num' }, [change(r.change_percent)]),
        el('td', { class: 'num opt', text: fmtCount(r.volume, r.currency === 'INR') }),
      ]));
      return el('div', { class: 'scroll' }, [el('table', {}, [
        el('caption', { class: 'sr-only', text: 'Watchlist with live prices' }),
        el('thead', {}, [el('tr', {}, [
          el('th', { scope: 'col', text: 'Symbol' }),
          el('th', { scope: 'col', class: 'num', text: 'Price' }),
          el('th', { scope: 'col', class: 'num', text: 'Change' }),
          el('th', { scope: 'col', class: 'num opt', text: 'Volume' }),
        ])]),
        el('tbody', {}, body),
      ])]);
    });
  }

  // `hide`: the class that drops a column from narrow cards (app.css).
  const HOLDING_COLUMNS = [
    { key: 'symbol', label: 'Symbol' },
    { key: 'shares', label: 'Qty', num: true, hide: 'opt', show: (h) => fmtQty(h.shares) },
    { key: 'avg_cost', label: 'Avg cost', num: true, hide: 'opt', show: (h) => fmtMoney(h.avg_cost, h.currency) },
    { key: 'price', label: 'Price', num: true, hide: 'opt-sm', show: (h) => fmtMoney(h.price, h.currency) },
    { key: 'value', label: 'Value', num: true, show: (h) => fmtMoney(h.value, h.currency) },
    { key: 'pnl', label: 'P&L', num: true, dir: true, show: (h) => fmtSignedMoney(h.pnl, h.currency) },
    { key: 'pnl_percent', label: 'P&L %', num: true, dir: true, show: (h) => fmtPct(h.pnl_percent) },
    { key: 'xirr_percent', label: 'XIRR', num: true, dir: true, show: (h) => fmtPct(h.xirr_percent, 1) },
    { key: 'weight_percent', label: 'Weight', num: true, hide: 'opt', show: (h) => (isNum(h.weight_percent) ? h.weight_percent.toFixed(1) + '%' : DASH) },
  ];

  function sortedHoldings(holdings) {
    const { key, dir } = state.sort;
    if (!key) return holdings;
    return holdings.slice().sort((a, b) => {
      const x = a[key];
      const y = b[key];
      // Missing values (no live price) always sink to the bottom.
      if (x == null || y == null) return (x == null) - (y == null);
      if (typeof x === 'string') return dir * x.localeCompare(y);
      return dir * (x - y);
    });
  }

  function onSort(key) {
    if (state.sort.key === key) state.sort.dir = -state.sort.dir;
    else state.sort = { key, dir: key === 'symbol' ? 1 : -1 };
    renderPortfolio();
    const btn = document.querySelector('#portfolio th button[data-sort="' + key + '"]');
    if (btn) btn.focus();
  }

  function stat(label, value, title, delta) {
    return el('div', { class: 'stat' }, [
      el('div', { class: 'label', text: label }),
      el('div', { class: 'value', title, text: value }),
      delta || null,
    ]);
  }

  function allocation(holdings, totalsCurrency) {
    const weighted = holdings
      .map((h, i) => ({ h, i }))
      .filter(({ h }) => isNum(h.weight_percent) && h.weight_percent > 0)
      .sort((a, b) => b.h.weight_percent - a.h.weight_percent);
    if (weighted.length === 0) return null;
    const fits = weighted.length <= ALLOC_SLOTS;
    const shown = fits ? weighted : weighted.slice(0, ALLOC_SLOTS - 1);
    const rest = fits ? [] : weighted.slice(ALLOC_SLOTS - 1);
    // A color follows its holding (portfolio order), not its rank.
    const slots = new Map(shown.slice().sort((a, b) => a.i - b.i).map((x, n) => [x.i, n]));
    const parts = shown.map(({ h, i }) => ({ label: h.symbol, weight: h.weight_percent, cls: 'c' + slots.get(i) }));
    if (rest.length) {
      const weight = rest.reduce((s, { h }) => s + h.weight_percent, 0);
      parts.push({ label: 'Other (' + rest.length + ')', weight, cls: 'c-other' });
    }
    const summary = parts.map((p) => p.label + ' ' + p.weight.toFixed(1) + '%').join(', ');
    const bar = el('div', { class: 'alloc', role: 'img', 'aria-label': 'Allocation by value in ' + (totalsCurrency || 'base currency') + ': ' + summary });
    for (const p of parts) {
      const seg = el('div', { class: 'seg ' + p.cls, title: p.label + ' ' + p.weight.toFixed(1) + '%' });
      seg.style.flexGrow = String(p.weight);
      seg.style.flexBasis = '0';
      bar.append(seg);
    }
    const legend = el('ul', { class: 'legend', 'aria-hidden': 'true' }, parts.map((p) => el('li', {}, [
      el('span', { class: 'swatch ' + p.cls }),
      p.label + ' ' + p.weight.toFixed(1) + '%',
    ])));
    return [el('h3', { class: 'sr-only', text: 'Allocation' }), bar, legend];
  }

  function renderPortfolio() {
    frame($('portfolio'), 'portfolio', (p) => {
      const holdings = p.holdings || [];
      if (holdings.length === 0) {
        return el('p', { class: 'empty' }, [
          'No holdings yet. Add a purchase with ', code('stockwise portfolio add TCS 10 3500'),
          ' or import from your broker with ', code('stockwise import kite holdings.csv'), '.',
        ]);
      }
      const t = p.totals || {};
      const cur = t.currency;
      const priced = holdings.filter((h) => isNum(h.price)).length;
      const wrap = el('div');
      wrap.append(el('div', { class: 'stats' }, [
        stat('Value', fmtCompact(t.value, cur), fmtMoney(t.value, cur)),
        stat('Invested', fmtCompact(t.cost_basis, cur), fmtMoney(t.cost_basis, cur)),
        stat('Profit / loss', (t.pnl > 0 ? '+' : '') + fmtCompact(t.pnl, cur), fmtSignedMoney(t.pnl, cur),
          el('div', { class: 'delta' }, [change(t.pnl_percent)])),
        stat('Holdings', String(holdings.length), null,
          priced < holdings.length ? el('div', { class: 'delta muted', text: priced + ' priced' }) : null),
      ]));
      const alloc = allocation(holdings, cur);
      if (alloc) wrap.append(...alloc);

      const head = el('tr', {}, HOLDING_COLUMNS.map((c) => {
        const sorted = state.sort.key === c.key;
        const btn = el('button', { type: 'button', 'data-sort': c.key }, [
          c.label,
          el('span', { 'aria-hidden': 'true', text: sorted ? (state.sort.dir > 0 ? '↑' : '↓') : '' }),
        ]);
        btn.addEventListener('click', () => onSort(c.key));
        return el('th', {
          scope: 'col',
          class: [c.num ? 'num' : '', c.hide || ''].join(' ').trim() || null,
          'aria-sort': sorted ? (state.sort.dir > 0 ? 'ascending' : 'descending') : null,
        }, [btn]);
      }));
      const rows = sortedHoldings(holdings).map((h) => el('tr', { class: h.symbol === state.symbol ? 'selected' : null },
        HOLDING_COLUMNS.map((c) => {
          if (c.key === 'symbol') return el('th', { scope: 'row' }, [holdingLabel(h)]);
          const cls = ['num', c.hide || '', c.dir ? direction(h[c.key]) : ''].join(' ').trim();
          return el('td', { class: cls, text: c.show(h) });
        })));
      const foot = el('tr', {}, HOLDING_COLUMNS.map((c) => {
        const cls = [c.num ? 'num' : '', c.hide || ''].join(' ').trim() || null;
        switch (c.key) {
          case 'symbol': return el('td', { text: 'Total' + (cur ? ' (' + cur + ')' : '') });
          case 'value': return el('td', { class: cls, text: fmtMoney(t.value, cur) });
          case 'pnl': return el('td', { class: cls + ' ' + direction(t.pnl), text: fmtSignedMoney(t.pnl, cur) });
          case 'pnl_percent': return el('td', { class: cls + ' ' + direction(t.pnl_percent), text: fmtPct(t.pnl_percent) });
          default: return el('td', { class: cls });
        }
      }));
      wrap.append(el('div', { class: 'scroll' }, [el('table', {}, [
        el('caption', { class: 'sr-only', text: 'Holdings valued at live prices; column headers sort the table' }),
        el('thead', {}, [head]),
        el('tbody', {}, rows),
        el('tfoot', {}, [foot]),
      ])]));
      if ((p.unpriced || []).length) {
        wrap.append(el('p', { class: 'note-line', text: 'No live price, left out of the totals: ' + p.unpriced.join(', ') }));
      }
      if ((p.unconverted || []).length) {
        wrap.append(el('p', { class: 'note-line', text: 'No exchange rate, left out of the totals: ' + p.unconverted.join(', ') }));
      }
      const navDates = [...new Set(holdings.map((h) => h.nav_date).filter(Boolean))];
      if (navDates.length) {
        wrap.append(el('p', { class: 'note-line', text: 'Mutual funds at AMFI\'s end-of-day NAV as of ' + navDates.join(', ') + '.' }));
      }
      (p.notes || []).forEach((n) => wrap.append(el('p', { class: 'note-line', text: n })));
      return wrap;
    });
  }

  const STATUS = {
    triggered: ['Triggered', 0],
    waiting: ['Waiting', 1],
    no_price: ['No price', 2],
  };

  function renderAlerts() {
    const rows = state.data.alerts;
    const fired = (rows || []).filter((a) => a.status === 'triggered').length;
    const count = $('alerts-count');
    count.hidden = fired === 0;
    count.textContent = fired + ' triggered';
    frame($('alerts'), 'alerts', (list) => {
      if (list.length === 0) {
        return el('p', { class: 'empty' }, ['No alerts. Add one with ', code('stockwise alert add TCS above 4200'), '.']);
      }
      const ordered = list.slice().sort((a, b) =>
        ((STATUS[a.status] || [0, 3])[1] - (STATUS[b.status] || [0, 3])[1]) || a.index - b.index);
      return el('ul', { class: 'alerts' }, ordered.map((a) => {
        const [label] = STATUS[a.status] || [String(a.status)];
        return el('li', {}, [
          el('div', {}, [
            symbolButton(a.symbol),
            el('div', { class: 'cond' }, [
              a.condition + ' ' + fmtMoney(a.target, a.currency),
              ' · now ' + fmtMoney(a.price, a.currency),
            ]),
          ]),
          el('span', { class: 'badge ' + (STATUS[a.status] ? a.status : ''), text: label }),
        ]);
      }));
    });
  }

  // ── Chart ──

  function buildPeriods() {
    const box = $('periods');
    box.replaceChildren(...PERIODS.map(([key, label, long]) => {
      // The visible label stays the accessible name (speech input); the long
      // form is a hint.
      const b = el('button', { type: 'button', 'data-period': key, title: long, 'aria-pressed': String(key === state.period), text: label });
      b.addEventListener('click', () => {
        if (state.period === key) return;
        state.period = key;
        state.hist = null;
        writeHash();
        box.querySelectorAll('button').forEach((x) => x.setAttribute('aria-pressed', String(x === b)));
        loadChart(true);
      });
      return b;
    }));
  }

  async function loadChart(withHistory) {
    const symbol = state.symbol;
    if (!symbol) {
      renderQuote();
      renderChart();
      return;
    }
    const seq = ++state.chartSeq;
    const period = state.period;
    $('chart').classList.add('stale');
    const quoteReq = api('/api/quote?' + query({ symbols: symbol, ...EXACT }));
    const histReq = withHistory
      ? api('/api/history/' + encodeURIComponent(symbol) + '?' + query({ period, ...EXACT }))
      : null;
    const [q, h] = await Promise.allSettled([quoteReq, histReq]);
    if (seq !== state.chartSeq) return; // a newer selection won
    for (const r of [q, h]) {
      if (r.status === 'rejected' && r.reason instanceof AuthError) {
        if (getToken()) setToken(null);
        openLogin(r.reason.message);
        return;
      }
    }
    state.quote = q.status === 'fulfilled' && Array.isArray(q.value) ? q.value[0] || null : state.quote;
    if (withHistory) {
      if (h.status === 'fulfilled' && h.value && Array.isArray(h.value.bars)) {
        state.hist = h.value;
        state.chartError = null;
      } else {
        state.hist = null;
        state.chartError = h.status === 'rejected' ? h.reason.message : 'No price history';
      }
    }
    $('chart').classList.remove('stale');
    renderQuote();
    renderChart();
  }

  function renderQuote() {
    const box = $('chart-quote');
    const q = state.quote;
    $('chart-title').textContent = state.symbol || 'Chart';
    $('chart-name').textContent = q ? q.longName || q.shortName || '' : '';
    if (!q) {
      box.replaceChildren();
      return;
    }
    const cur = q.currency;
    box.replaceChildren(
      el('div', { class: 'price', text: fmtMoney(q.regularMarketPrice, cur) }),
      el('div', { class: 'small' }, [change(q.regularMarketChangePercent, fmtSignedMoney(q.regularMarketChange, cur))]),
      isNum(q.regularMarketDayLow) && isNum(q.regularMarketDayHigh)
        ? el('div', { class: 'small muted', text: 'Day ' + fmtNumber(q.regularMarketDayLow, cur) + ' – ' + fmtNumber(q.regularMarketDayHigh, cur) })
        : null,
    );
  }

  // Round tick values covering [min, max].
  function niceTicks(min, max, count) {
    if (min === max) {
      const pad = Math.abs(min) * 0.01 || 1;
      min -= pad;
      max += pad;
    }
    const raw = (max - min) / count;
    const mag = 10 ** Math.floor(Math.log10(raw));
    const norm = raw / mag;
    const step = (norm < 1.5 ? 1 : norm < 3 ? 2 : norm < 7 ? 5 : 10) * mag;
    const ticks = [];
    for (let v = Math.floor(min / step) * step; v <= max + step * 0.5; v += step) {
      ticks.push(Number(v.toFixed(10)));
      if (v >= max) break;
    }
    return { ticks, step };
  }

  function dateFormat(period) {
    if (period === '1d') return new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' });
    if (period === '5d' || period === '1w') return new Intl.DateTimeFormat(undefined, { weekday: 'short', day: 'numeric' });
    if (['1y', '2y', '5y', '10y', 'max'].includes(period)) return new Intl.DateTimeFormat(undefined, { month: 'short', year: '2-digit' });
    return new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short' });
  }

  const longDate = (period) => new Intl.DateTimeFormat(undefined, period === '1d' || period === '5d'
    ? { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' }
    : { day: 'numeric', month: 'short', year: 'numeric' });

  let chartView = null;

  function renderChart() {
    const box = $('chart');
    chartView = null;
    const hist = state.hist;
    if (!state.symbol) {
      box.replaceChildren(el('p', { class: 'empty', text: 'Pick a symbol from the watchlist, the portfolio or search.' }));
      return;
    }
    if (!hist) {
      box.replaceChildren(el('p', { class: state.chartError ? 'empty error' : 'empty', text: state.chartError || 'Loading chart…' }));
      return;
    }
    const pts = hist.bars
      .filter((b) => isNum(b.close))
      .map((b) => ({ t: new Date(b.time), close: b.close, open: b.open, high: b.high, low: b.low, volume: b.volume }));
    if (pts.length < 2) {
      box.replaceChildren(el('p', { class: 'empty', text: 'Not enough price history for this period.' }));
      return;
    }
    const cur = hist.currency;
    const W = Math.max(box.clientWidth, 260);
    const H = Math.max(box.clientHeight, 180);
    const closes = pts.map((p) => p.close);
    const lo = Math.min(...closes);
    const hi = Math.max(...closes);
    const { ticks, step } = niceTicks(lo, hi, H < 250 ? 3 : 4);
    const digits = step < 1 ? Math.min(6, Math.ceil(-Math.log10(step))) : 0;
    const labels = ticks.map((v) => fmtNumber(v, cur, digits));
    const pad = { l: 4, r: 12 + Math.max(...labels.map((s) => s.length)) * 6.6, t: 10, b: 24 };
    const plotW = W - pad.l - pad.r;
    const plotH = H - pad.t - pad.b;
    const y0 = ticks[0];
    const y1 = ticks[ticks.length - 1];
    const x = (i) => pad.l + (i / (pts.length - 1)) * plotW;
    const y = (v) => pad.t + (1 - (v - y0) / (y1 - y0)) * plotH;

    const first = closes[0];
    const last = closes[closes.length - 1];
    const periodName = (PERIODS.find(([k]) => k === hist.period) || [0, 0, hist.period])[2];
    const summary = state.symbol + ', ' + periodName + ': ' + fmtMoney(first, cur) + ' to ' + fmtMoney(last, cur) +
      ' (' + fmtPct(((last - first) / first) * 100) + '). Low ' + fmtMoney(lo, cur) + ', high ' + fmtMoney(hi, cur) + '.';

    const root = svgEl('svg', { viewBox: '0 0 ' + W + ' ' + H, role: 'img', 'aria-label': summary, preserveAspectRatio: 'none' });
    root.append(svgEl('title', { text: summary }));
    ticks.forEach((v, i) => {
      const yy = y(v);
      root.append(svgEl('line', { class: 'gridline', x1: pad.l, x2: pad.l + plotW, y1: yy, y2: yy }));
      root.append(svgEl('text', { class: 'tick', x: W - 4, y: yy + 4, 'text-anchor': 'end', text: labels[i] }));
    });
    const fmt = dateFormat(hist.period);
    const xTicks = Math.max(2, Math.min(6, Math.floor(plotW / 110)));
    for (let k = 0; k < xTicks; k++) {
      const i = Math.round((k / (xTicks - 1)) * (pts.length - 1));
      const anchor = k === 0 ? 'start' : k === xTicks - 1 ? 'end' : 'middle';
      root.append(svgEl('text', { class: 'tick', x: x(i), y: H - 6, 'text-anchor': anchor, text: fmt.format(pts[i].t) }));
    }
    const line = pts.map((p, i) => (i ? 'L' : 'M') + x(i).toFixed(1) + ' ' + y(p.close).toFixed(1)).join(' ');
    const bottom = (pad.t + plotH).toFixed(1);
    root.append(svgEl('path', { class: 'area', d: line + ' L' + x(pts.length - 1).toFixed(1) + ' ' + bottom + ' L' + x(0).toFixed(1) + ' ' + bottom + ' Z' }));
    root.append(svgEl('path', { class: 'line', d: line }));
    root.append(svgEl('circle', { class: 'dot', cx: x(pts.length - 1), cy: y(last), r: 4 }));
    const cross = svgEl('line', { class: 'cross', y1: pad.t, y2: pad.t + plotH, visibility: 'hidden' });
    const marker = svgEl('circle', { class: 'dot', r: 4, visibility: 'hidden' });
    root.append(cross, marker);
    const tip = el('div', { class: 'tip', hidden: true, 'aria-hidden': 'true' });
    box.replaceChildren(root, tip);
    chartView = { pts, x, y, W, plotW, pad, cur, cross, marker, tip, root, index: pts.length - 1, long: longDate(hist.period) };
  }

  function pointText(p) {
    const v = chartView;
    return v.long.format(p.t) + ': close ' + fmtMoney(p.close, v.cur) +
      (isNum(p.high) && isNum(p.low) ? ', high ' + fmtNumber(p.high, v.cur) + ', low ' + fmtNumber(p.low, v.cur) : '');
  }

  function showPoint(i, announce) {
    const v = chartView;
    if (!v) return;
    v.index = Math.max(0, Math.min(v.pts.length - 1, i));
    const p = v.pts[v.index];
    const px = v.x(v.index);
    v.cross.setAttribute('x1', px);
    v.cross.setAttribute('x2', px);
    v.cross.setAttribute('visibility', 'visible');
    v.marker.setAttribute('cx', px);
    v.marker.setAttribute('cy', v.y(p.close));
    v.marker.setAttribute('visibility', 'visible');
    v.tip.replaceChildren(
      el('strong', { text: fmtMoney(p.close, v.cur) }),
      el('span', { text: v.long.format(p.t) }),
      isNum(p.volume) ? el('span', { text: ' · vol ' + fmtCount(p.volume, v.cur === 'INR') }) : null,
    );
    v.tip.hidden = false;
    // Keep the tooltip inside the chart, on the far side of the pointer.
    const shown = v.root.getBoundingClientRect().width || v.W;
    const left = px * (shown / v.W);
    const width = v.tip.offsetWidth;
    v.tip.style.left = (left + 12 + width > shown ? Math.max(0, left - 12 - width) : left + 12) + 'px';
    if (announce) $('chart-readout').textContent = pointText(p);
  }

  function hidePoint() {
    const v = chartView;
    if (!v) return;
    v.cross.setAttribute('visibility', 'hidden');
    v.marker.setAttribute('visibility', 'hidden');
    v.tip.hidden = true;
  }

  function onChartPointer(ev) {
    const v = chartView;
    if (!v) return;
    const rect = v.root.getBoundingClientRect();
    const px = ((ev.clientX - rect.left) / rect.width) * v.W;
    showPoint(Math.round(((px - v.pad.l) / v.plotW) * (v.pts.length - 1)), false);
  }

  function onChartKey(ev) {
    const v = chartView;
    if (!v) return;
    const big = Math.max(1, Math.round(v.pts.length / 10));
    const moves = {
      ArrowLeft: v.index - 1,
      ArrowRight: v.index + 1,
      PageUp: v.index - big,
      PageDown: v.index + big,
      Home: 0,
      End: v.pts.length - 1,
    };
    if (ev.key in moves) {
      ev.preventDefault();
      showPoint(moves[ev.key], true);
    } else if (ev.key === 'Escape') {
      hidePoint();
    }
  }

  // ── Search ──

  const search = { seq: 0, timer: null, results: [], active: -1, pending: null };

  function closeResults() {
    const list = $('search-results');
    list.hidden = true;
    list.replaceChildren();
    search.results = [];
    search.active = -1;
    $('search').setAttribute('aria-expanded', 'false');
    $('search').removeAttribute('aria-activedescendant');
  }

  function showResults(items, note) {
    const list = $('search-results');
    search.results = items;
    search.active = items.length ? 0 : -1;
    const nodes = items.map((r, i) => {
      const li = el('li', { id: 'search-opt-' + i, role: 'option', 'aria-selected': String(i === 0) }, [
        el('span', { class: 'sym', text: r.symbol }),
        el('span', { class: 'meta', text: [r.shortname || r.longname, r.exchange].filter(Boolean).join(' · ') }),
      ]);
      li.addEventListener('mousedown', (ev) => ev.preventDefault());
      li.addEventListener('click', () => choose(i));
      return li;
    });
    if (note) nodes.push(el('li', { class: 'note', role: 'presentation', text: note }));
    list.replaceChildren(...nodes);
    list.hidden = nodes.length === 0;
    $('search').setAttribute('aria-expanded', String(!list.hidden));
    setActive(search.active);
  }

  function setActive(i) {
    const input = $('search');
    search.active = i;
    $('search-results').querySelectorAll('[role="option"]').forEach((li, n) => {
      li.setAttribute('aria-selected', String(n === i));
      if (n === i) li.scrollIntoView({ block: 'nearest' });
    });
    if (i >= 0) input.setAttribute('aria-activedescendant', 'search-opt-' + i);
    else input.removeAttribute('aria-activedescendant');
  }

  function runSearch() {
    clearTimeout(search.timer);
    search.timer = null;
    const q = $('search').value.trim();
    if (!q) {
      closeResults();
      return Promise.resolve();
    }
    const seq = ++search.seq;
    search.pending = api('/api/search?' + query({ q })).then((rows) => {
      if (seq !== search.seq) return;
      const items = (Array.isArray(rows) ? rows : []).filter((r) => r && r.symbol && SYMBOL_RE.test(r.symbol)).slice(0, 8);
      showResults(items, items.length ? null : 'No matches for “' + q + '”');
    }, (e) => {
      if (seq !== search.seq) return;
      if (e instanceof AuthError) {
        closeResults();
        openLogin(e.message);
      } else {
        showResults([], 'Search failed: ' + e.message);
      }
    });
    return search.pending;
  }

  function choose(i) {
    const r = search.results[i];
    if (!r) return;
    $('search').value = '';
    closeResults();
    selectSymbol(r.symbol, true);
    $('chart-readout').textContent = 'Showing ' + r.symbol;
  }

  async function onSearchKey(ev) {
    const n = search.results.length;
    if (ev.key === 'ArrowDown' && n) {
      ev.preventDefault();
      setActive((search.active + 1) % n);
    } else if (ev.key === 'ArrowUp' && n) {
      ev.preventDefault();
      setActive((search.active - 1 + n) % n);
    } else if (ev.key === 'Enter') {
      ev.preventDefault();
      if (!n || search.timer) await runSearch();
      choose(search.active >= 0 ? search.active : 0);
    } else if (ev.key === 'Escape') {
      if (!$('search-results').hidden) {
        ev.preventDefault();
        closeResults();
      } else {
        $('search').value = '';
      }
    }
  }

  function onSearchInput() {
    clearTimeout(search.timer);
    search.timer = setTimeout(() => {
      search.timer = null;
      runSearch();
    }, 250);
  }

  // ── Start ──

  function start() {
    readHash();
    buildPeriods();
    $('signout').hidden = !getToken();

    $('refresh').addEventListener('click', () => (state.needsToken ? openLogin('') : refreshAll(true)));
    $('signout').addEventListener('click', signOut);
    $('banner-signin').addEventListener('click', () => openLogin(''));
    $('login-form').addEventListener('submit', onLogin);
    $('login-cancel').addEventListener('click', () => $('login').close());
    $('base').addEventListener('change', async (ev) => {
      state.base = ev.target.value;
      try {
        await loadSection('portfolio', portfolioPath());
      } catch (e) {
        if (e instanceof AuthError) openLogin(e.message);
      }
      renderPortfolio();
    });
    for (const id of ['indices', 'watchlist', 'portfolio', 'alerts']) $(id).addEventListener('click', onSymbolClick);

    const chart = $('chart');
    chart.addEventListener('pointermove', onChartPointer);
    chart.addEventListener('pointerleave', () => {
      if (document.activeElement !== chart) hidePoint();
    });
    chart.addEventListener('keydown', onChartKey);
    chart.addEventListener('focus', () => chartView && showPoint(chartView.index, true));
    chart.addEventListener('blur', hidePoint);
    let lastWidth = chart.clientWidth;
    new ResizeObserver(() => {
      if (chart.clientWidth !== lastWidth) {
        lastWidth = chart.clientWidth;
        renderChart();
      }
    }).observe(chart);

    const input = $('search');
    input.addEventListener('input', onSearchInput);
    input.addEventListener('keydown', onSearchKey);
    input.addEventListener('blur', closeResults);
    $('search-form').addEventListener('submit', (ev) => ev.preventDefault());

    document.addEventListener('visibilitychange', onVisibility);
    window.addEventListener('hashchange', () => {
      const before = state.symbol + state.period;
      readHash();
      if (state.symbol + state.period !== before) {
        buildPeriods();
        state.hist = null;
        renderAll();
        loadChart(true);
      }
    });

    renderAll();
    renderQuote();
    renderChart();
    refreshAll(true);
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start);
  else start();
})();
