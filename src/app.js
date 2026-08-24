// Tuxtop frontend logic.
//
// Loaded as an external file, never inline. Tauri injects a nonce into the
// page's script-src, and per the CSP spec a nonce makes the browser ignore
// 'unsafe-inline' - so an inline <script> is silently blocked and the whole
// UI goes dead with no visible error. External files are covered by
// script-src 'self' and work under any CSP.
//
(() => {
  // Pure logic lives in modules beside this file so it can be tested; app.js
  // keeps the DOM. Bound here rather than referenced as TuxScale.band(...)
  // everywhere, so the call sites read the same as before the extraction.
  const { bps, gb, fmtKb, fmtSpan, humanUptime, shortGpu } = TuxFormat;
  const { band, logWindow, normalise, sliderToSecs, niceCols } = TuxScale;
  const { fullestFs, fsPct, sensorName, sensorMetric, hottestSensor,
          machine, machineLabel, stealIsMeaningful } = TuxPick;
  const { matchesHost, matchesProcess } = TuxFilter;
  const { heatRow, coverage, heatOrder, groupBreaks, ramp, mixHex } = TuxHeat;

  const $ = s => document.querySelector(s);
  const grid = $('#grid');
  /// The one mode class each view puts on the grid. build() toggles all of
  /// them from this map, so a new view cannot forget its own teardown.
  const MODE_CLASS = { all: 'all-mode', history: 'hist-mode',
                       procs: 'proc-mode', heat: 'heat-mode' };
  const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;

  // ---- model: real hosts from the tailnet, real core counts ----
  let seq = 0;
  const mk = (name, distro, cores, ramGB, gpu, base) => ({
    group: null,
    id: ++seq, name, distro, cores, ramGB, gpu, base,
    core: Array.from({length: cores}, () => Math.random() * 4),
    burst: 0, burstSet: [],
    ram: ramGB * (0.14 + Math.random() * 0.12),
    hist: { cpu: [], ram: [], dio: [], net: [], gpu: [], temp: [] },
    net: 0.4, dio: 2, gpuU: 0
  });

  // Under Tauri the host list comes from hosts.toml and every number arrives
  // from the backend. Opened as a plain file there is no backend, so the
  // simulator below runs against these instead -- that keeps this page usable
  // as the design mockup it started life as.
  const TAURI = globalThis.__TAURI__;
  const LIVE = !!TAURI;

  let hosts = LIVE ? [] : [
    mk('dove',   'Debian 13', 32, 31,  'RTX 3080', 3),
    mk('heron',  'Debian 12',  8, 16,  null,       11),
    mk('wader',  'Debian 11',  4,  8,  null,       6),
    mk('falcon', 'Ubuntu 24',  8, 32,  null,       2),
  ];

  // View preferences, remembered between launches.
  const PREFS = 'tuxtop.prefs';
  const prefs = Object.assign(
    { view: 'hosts', sort: 'manual', metric: 'cores', slice: null,
      procKernel: false, procSort: 'cpu_pct', procDesc: true, procByOwner: false,
      // Slider position, not seconds: the mapping is log-spaced, and storing
      // the position keeps a restored window identical to the one that was
      // set rather than the nearest step to a rounded number of seconds.
      // 0 is the minimum, 60 s - the live end, which is where someone opening
      // History from a card they are watching almost always wants to be.
      histWin: 0,
      // Heat excludes vector metrics, so it cannot always honour the Fleet
      // view's choice. Its own key keeps switching views from silently
      // rewriting the other's.
      heatMetric: 'cpu' },
    JSON.parse(localStorage.getItem(PREFS) || '{}')
  );
  const savePrefs = () => localStorage.setItem(PREFS, JSON.stringify(prefs));

  // One fixed core-tile size everywhere, in every fleet, on every host.
  const TILE_PX = 34;

  const HIST = 60;
  const push = (a, v) => { a.push(v); if (a.length > HIST) a.shift(); };
  const clamp = (v, a, b) => v < a ? a : v > b ? b : v;
  const css = n => getComputedStyle(document.documentElement).getPropertyValue(n).trim();

  // ---- simulation ----
  function step(h) {
    if (h.burst > 0) h.burst--;
    else if (Math.random() < 0.055) {
      h.burst = 3 + (Math.random() * 6 | 0);
      const k = 1 + (Math.random() * Math.max(1, h.cores * 0.55) | 0);
      h.burstSet = Array.from({length: k}, () => Math.random() * h.cores | 0);
    }
    for (let i = 0; i < h.cores; i++) {
      const hot = h.burst > 0 && h.burstSet.includes(i);
      const target = hot ? 62 + Math.random() * 38 : h.base + Math.random() * 9;
      h.core[i] = clamp(h.core[i] + (target - h.core[i]) * 0.42 + (Math.random() - 0.5) * 5, 0, 100);
    }
    const cpu = h.core.reduce((a, b) => a + b, 0) / h.cores;
    h.ram = clamp(h.ram + (Math.random() - 0.48) * 0.22, h.ramGB * 0.08, h.ramGB * 0.82);
    h.dio = clamp(h.dio + (Math.random() - 0.5) * 18 + (h.burst > 0 ? 9 : -2), 0, 260);
    h.net = clamp(h.net + (Math.random() - 0.5) * 1.5 + (h.burst > 0 ? .7 : -.15), 0, 42);
    if (h.gpu) h.gpuU = clamp(h.gpuU * 0.86 + (h.burst > 0 ? Math.random() * 42 : Math.random() * 5), 0, 100);
    push(h.hist.cpu, cpu);
    push(h.hist.ram, h.ram / h.ramGB * 100);
    push(h.hist.dio, h.dio);
    push(h.hist.net, h.net);
    push(h.hist.gpu, h.gpuU);
    return cpu;
  }

  // ---- canvas ----
  function draw(cv, data, color, max) {
    const dpr = devicePixelRatio || 1;
    const w = cv.clientWidth, hgt = cv.clientHeight;
    if (!w || !hgt) return;
    if (cv.width !== w * dpr || cv.height !== hgt * dpr) { cv.width = w * dpr; cv.height = hgt * dpr; }
    const c = cv.getContext('2d');
    c.setTransform(dpr, 0, 0, dpr, 0, 0);
    c.clearRect(0, 0, w, hgt);
    if (data.length < 2) return;

    const peak = max || Math.max(10, ...data) * 1.15;
    const x = i => i / (HIST - 1) * w;
    const y = v => hgt - (clamp(v, 0, peak) / peak) * (hgt - 3) - 1.5;
    const off = HIST - data.length;

    c.strokeStyle = css('--stroke');
    c.lineWidth = 1;
    c.beginPath(); c.moveTo(0, hgt / 2); c.lineTo(w, hgt / 2); c.stroke();

    c.beginPath();
    c.moveTo(x(off), hgt);
    data.forEach((v, i) => c.lineTo(x(i + off), y(v)));
    c.lineTo(x(data.length - 1 + off), hgt);
    c.closePath();
    const g = c.createLinearGradient(0, 0, 0, hgt);
    g.addColorStop(0, color + '66');
    g.addColorStop(1, color + '03');
    c.fillStyle = g; c.fill();

    c.beginPath();
    data.forEach((v, i) => i ? c.lineTo(x(i + off), y(v)) : c.moveTo(x(i + off), y(v)));
    c.strokeStyle = color; c.lineWidth = 1.6;
    c.lineJoin = 'round'; c.stroke();

    const lx = x(data.length - 1 + off), ly = y(data[data.length - 1]);
    c.beginPath(); c.arc(lx, ly, 2.6, 0, 7); c.fillStyle = color; c.fill();
    c.beginPath(); c.arc(lx, ly, 5.2, 0, 7); c.strokeStyle = color + '55'; c.lineWidth = 1.4; c.stroke();
  }


  // ---- groups -------------------------------------------------------------
  //
  // Aggregation itself lives in agg.js under ADR-008. This is only the part
  // that turns hosts into the shape it expects and rows into DOM.

  const groupOpen = name => !!(prefs.groupOpen && prefs.groupOpen[name]);

  function toggleGroup(name) {
    prefs.groupOpen = prefs.groupOpen || {};
    prefs.groupOpen[name] = !prefs.groupOpen[name];
    savePrefs();
    build();
    paint();
  }

  /// The fleet as a flat list of rows: each group, its members if it is
  /// expanded, then the hosts that belong to no group.
  ///
  /// With no groups configured this returns exactly `ordered()`, so a fleet
  /// that has never used the feature renders precisely as it did before it
  /// existed.
  function fleetRows() {
    const { groups, ungrouped } = TuxAgg.groupHosts(visible());
    const rows = [];
    for (const g of groups) {
      rows.push({ kind: 'group', name: g.name, hosts: g.hosts });
      if (groupOpen(g.name)) {
        for (const h of g.hosts) rows.push({ kind: 'host', host: h, member: true });
      }
    }
    for (const h of ungrouped) rows.push({ kind: 'host', host: h });
    return rows;
  }

  /// A group's hosts in the flat shape `aggregateGroup` expects.
  ///
  /// A faulted host, or one that does not report this metric at all,
  /// contributes `null` rather than a zero. Zero would be a reading; silence
  /// is not, and the difference is the whole point of the contributing count.
  function members(m, list) {
    return list.map(h => {
      const silent = !!h.fault || (m.has && !m.has(h));
      const v = silent ? null : m.scalar(h);
      return {
        host: h.name,
        value: v === undefined ? null : v,
        parts: !silent && m.parts ? m.parts(h) : null,
        vector: !silent && m.vector ? m.vector(h) : null,
      };
    });
  }

  /// What a group's row says about itself under the value.
  ///
  /// States composition always, and states partiality whenever it exists: a
  /// group summarising four hosts while appearing to summarise five is the
  /// averaged-away spike one level up.
  function groupSub(g, hosts, m) {
    const parts = [];

    // For a 'max' aggregate the group's value *is* one member's reading, so
    // that member is named. A group reading 71C while declining to say which
    // host - or which component - is the aggregate hiding a member, which
    // ADR-008 exists to forbid.
    if (m && m.agg === 'max' && g && g.top) {
      const h = hosts.find(x => x.name === g.top.host);
      const detail = h && m.sub ? m.sub(h) : '';
      parts.push(detail ? `${g.top.host} · ${detail}` : g.top.host);
    } else {
      const cores = hosts.reduce((a, h) => a + (h.cores || 0), 0);
      const n = hosts.length;
      parts.push(`${n} host${n === 1 ? '' : 's'}`);
      if (cores) parts.push(`${cores} cores`);
    }

    if (g && g.partial) parts.push(`${g.contributing} of ${g.total} reporting`);
    return parts.join(' · ');
  }

  // ---- render skeleton ----
  /// Hosts in display order. 'manual' is whatever order the backend holds,
  /// which is what dragging persists -- so sorting is a view over it, never a
  /// mutation of it.
  function ordered() {
    const list = hosts.slice();
    if (prefs.sort === 'load') {
      // "Busiest" means the metric on screen, not always CPU - in Heat that is
      // its own pref. Read from METRICS directly rather than calling
      // heatMetric(), which is declared further down: ordered() is reachable
      // from module-scope startup, and a const arrow referenced before its
      // initialiser throws in the temporal dead zone. That exact shape once
      // brought the whole UI up inert with one console line.
      const m = prefs.view === 'all' ? metric()
        : prefs.view === 'heat' ? (METRICS[prefs.heatMetric] || METRICS.cpu)
        : METRICS.cpu;
      return list.sort((a, b) => (m.scalar(b) || 0) - (m.scalar(a) || 0));
    }
    if (prefs.sort === 'name') {
      return list.sort((a, b) => a.name.localeCompare(b.name));
    }
    return list;
  }

  /// Ad-hoc "show me these hosts", shared by the Hosts, Fleet and History
  /// views. Groups are the durable structure; this is the transient question.
  ///
  /// Deliberately not persisted, and for a sharper reason than the process
  /// filter. The Fleet view is what you glance at to confirm the fleet is
  /// fine. A filter restored on Monday morning would show eight calm bars
  /// while eleven hidden hosts were on fire - a filtered view that looks like
  /// a complete one, which is the failure this whole project guards against.
  let hostFilter = '';

  /// Hosts to draw: ordered, then filtered.
  ///
  /// Separate from `ordered()` on purpose. That one feeds drag-reorder
  /// persistence and the default history subject, and filtering it would drop
  /// hidden hosts from the saved order and hand an empty list to code that
  /// assumes at least one host exists.
  function visible() {
    const q = hostFilter.trim().toLowerCase();
    return q ? ordered().filter(h => matchesHost(h, q)) : ordered();
  }

  /// "showing 3 of 19 hosts", or nothing when everything is shown.
  function filterNote() {
    const q = hostFilter.trim().toLowerCase();
    if (!q) return '';
    const n = visible().length;
    return n
      ? `showing ${n} of ${hosts.length} hosts`
      : `no host matches \u201c${hostFilter.trim()}\u201d`;
  }

  /// State how much is hidden, and shout about it in the Fleet view.
  ///
  /// Everywhere else a filtered view is obviously a search result. The Fleet
  /// view is the one you glance at to confirm nothing is wrong, so a filtered
  /// one must not read as a healthy one - hence a warning tone there and a
  /// quiet one elsewhere. Same information, different urgency.
  function showFilterNote() {
    const el = $('#filterNote');
    if (!el) return;
    const text = filterNote();
    el.hidden = !text || prefs.view === 'procs';
    el.textContent = text;
    el.dataset.tone = prefs.view === 'all' ? 'loud' : 'quiet';
  }

  const last = a => (a.length ? a[a.length - 1] : 0);

  // ---- metric registry ----------------------------------------------------
  //
  // The fleet is a matrix of hosts x metrics. A host card is a row of it; the
  // fleet view is a column. Every metric declares its shape and its scale, so
  // adding one is a table entry rather than a new renderer.
  //
  //   shape 'vector' - one value per core/disk/nic; drawn as a tile grid
  //   shape 'scalar' - one value per host; drawn as a comparable bar
  //
  //   scale 'absolute' - already a percentage, so 0..100 is meaningful and
  //                      shared across hosts without transformation
  //   scale 'log'      - a rate. A linear axis lets one busy host flatten
  //                      everyone else into invisible slivers, while
  //                      normalising per host hides that dove moves 10x the
  //                      traffic heron does. Log keeps magnitude and
  //                      visibility at once.
  const METRICS = {
    cores: {
      label: 'CPU cores', shape: 'vector', scale: 'absolute', max: 100,
      agg: 'concat',
      vector: h => h.core || [],
      scalar: h => last(h.hist.cpu),
      fmt: v => Math.round(v) + '%',
      has: h => (h.cores || 0) > 0,
    },
    cpu: {
      label: 'CPU', shape: 'scalar', scale: 'absolute', max: 100,
      // Busy core-time over total core-time. A 32-core host is not one vote.
      agg: 'ratio', parts: h => [last(h.hist.cpu) * (h.cores || 0), h.cores || 0],
      scalar: h => last(h.hist.cpu),
      fmt: v => Math.round(v) + '%',
    },
    mem: {
      label: 'Memory', shape: 'scalar', scale: 'absolute', max: 100,
      agg: 'ratio', parts: h => [(h.ram || 0) * 100, h.ramGB || 0],
      scalar: h => (h.ramGB ? h.ram / h.ramGB * 100 : 0),
      fmt: v => Math.round(v) + '%',
      sub: h => `${gb(h.ram)} / ${gb(h.ramGB)} GB`,
    },
    temp: {
      // Absolute, not log: degrees are already a bounded, directly comparable
      // scale. 100C is the conventional throttle point, so the bar reads as
      // "fraction of the way to too hot".
      label: 'CPU temp', shape: 'scalar', scale: 'absolute', max: 100,
      // Max, never a mean: four cool hosts must not cool a throttling one.
      agg: 'max',
      scalar: h => (h.temp ?? null),
      fmt: v => Math.round(v) + '\u00b0C',
      has: h => h.temp !== null && h.temp !== undefined,
    },
    hottest: {
      // Separate from 'CPU temp' on purpose. Merging them would mean either
      // reporting an NVMe as the CPU, or hiding the hottest thing in the box.
      label: 'Hottest sensor', shape: 'scalar', scale: 'absolute', max: 100,
      agg: 'max',
      scalar: h => { const t = hottestSensor(h); return t ? t.celsius : null; },
      fmt: v => Math.round(v) + '\u00b0C',
      // Always names the component: the number is not actionable without it.
      sub: h => { const t = hottestSensor(h); return t ? t.name : ''; },
      has: h => !!hottestSensor(h),
    },
    fs: {
      label: 'Disk usage', shape: 'scalar', scale: 'absolute', max: 100,
      // One full disk is the problem, not the group's mean fullness.
      agg: 'max',
      scalar: h => fsPct(h),
      fmt: v => Math.round(v) + '%',
      sub: h => {
        const f = fullestFs(h);
        return f ? `${f.mount}  ${gb(f.used_kb / 1048576)}/${gb(f.total_kb / 1048576)}G` : '';
      },
      has: h => fsPct(h) !== null,
    },
    swap: {
      label: 'Swap', shape: 'scalar', scale: 'absolute', max: 100,
      agg: 'max',
      scalar: h => (h.swapTotal ? h.swapUsed / h.swapTotal * 100 : null),
      fmt: v => Math.round(v) + '%',
      sub: h => (h.swapTotal ? `${gb(h.swapUsed / 1048576)}/${gb(h.swapTotal / 1048576)}G` : ''),
      // A host with no swap configured is normal, not a host with 0% swap.
      has: h => h.swapTotal > 0,
    },
    disk: {
      label: 'Disk I/O', shape: 'scalar', scale: 'log', floor: 1e6, decades: 4,
      agg: 'sum',
      scalar: h => h.dio || 0, fmt: bps,
    },
    net: {
      label: 'Network', shape: 'scalar', scale: 'log', floor: 1e6, decades: 4,
      agg: 'sum',
      scalar: h => h.net || 0, fmt: bps,
    },
    load: {
      label: 'Load avg', shape: 'scalar', scale: 'log', floor: 1, decades: 3,
      // Runnable processes add. Log scale already carries the magnitude.
      agg: 'sum',
      scalar: h => (h.load ? h.load[0] : 0), fmt: v => v.toFixed(2),
    },
    // GPU needs nvidia-smi in the sampler loop, which is Phase 6. Until a
    // host actually reports it these stay out of the picker entirely: a
    // metric that renders 0% everywhere because nothing collects it is a
    // confidently wrong number, which is the one thing this app must not do.
    gpu: {
      label: 'GPU load', shape: 'scalar', scale: 'absolute', max: 100,
      // Weighted by GPU count, which is one per host today.
      agg: 'ratio', parts: h => [(h.gpu ? h.gpuU || 0 : 0), 1],
      scalar: h => (h.gpu ? h.gpuU || 0 : null), fmt: v => Math.round(v) + '%',
      has: h => !!h.gpu,
    },
    gpumem: {
      label: 'GPU memory', shape: 'scalar', scale: 'absolute', max: 100,
      agg: 'ratio', parts: h => [(h.gpuUsed || 0) * 100, h.gpuTotal || 0],
      scalar: h => (h.gpu && h.gpuTotal ? h.gpuUsed / h.gpuTotal * 100 : null),
      fmt: v => Math.round(v) + '%',
      sub: h => (h.gpuTotal ? `${Math.round(h.gpuUsed)} / ${Math.round(h.gpuTotal)} MB` : ''),
      has: h => !!(h.gpu && h.gpuTotal),
    },
  };

  const metric = () => METRICS[prefs.metric] || METRICS.cores;

  /// Metrics at least one host actually reports.
  ///
  /// A metric with no data behind it would render zeros across the fleet and
  /// look exactly like a genuinely idle one. Hiding it is the honest option;
  /// it reappears the moment a host reports it.
  const availableMetrics = () => {
    // Only hosts that have actually reported can testify to what exists.
    // Cards seeded from hosts.toml start with cores: 0 before their first
    // sample, so judging availability on all hosts declared every gated
    // metric missing and overwrote the saved preference on the way to the
    // first sample. Absence of data is not evidence of an absent metric.
    const reporting = hosts.filter(h => h.seen || !LIVE);
    if (!reporting.length) return Object.entries(METRICS);
    return Object.entries(METRICS).filter(([, m]) => !m.has || reporting.some(h => m.has(h)));
  };

  // Suspends repaints while a drag is in flight.
  let dragging = null;
  let frozen = false;

  /// Move `name` next to `target` and persist the arrangement.
  ///
  /// Dragging implies manual ordering, so an active sort is switched off
  /// rather than silently discarding the drop the user just made.
  function commitOrder(name, target, after) {
    if (!name || name === target) return;

    const order = ordered().map(h => h.name).filter(n => n !== name);
    const at = order.indexOf(target);
    order.splice(at + (after ? 1 : 0), 0, name);

    if (prefs.sort !== 'manual') {
      prefs.sort = 'manual';
      savePrefs();
      const sel = document.querySelector('#sortSel');
      if (sel) sel.value = 'manual';
    }

    // Apply locally first so the card lands where it was dropped, without
    // waiting for a round trip.
    const byName = new Map(hosts.map(h => [h.name, h]));
    hosts = order.map(n => byName.get(n)).filter(Boolean);
    build(); paint();

    if (LIVE) {
      TAURI.core.invoke('reorder_hosts', { names: order })
        .catch(err => showError(`Could not save order: ${err}`));
    }
  }

  // ---- all-cores view -----------------------------------------------------
  //
  // Every core of every host on one grid: pure load, nothing else. Tiles size
  // themselves to the total core count so the whole fleet fits the window
  // rather than scrolling -- with 52 cores across four boxes, the point is
  // seeing them all at once.
  // ---- fleet view: one metric, every host --------------------------------
  function buildFleet() {
    grid.innerHTML = '';
    const m = metric();
    grid.dataset.shape = m.shape;
    grid.dataset.scale = m.scale;
    grid.style.setProperty('--dec', m.decades || 4);

    // The load bands mean nothing on a rate metric - there is no "too hot"
    // for bytes per second - so the legend that explains them is hidden
    // rather than left on screen asserting a scale that is not in use.
    document.body.dataset.bands = m.scale === 'absolute' ? 'on' : 'off';

    if (!hosts.length) {
      grid.innerHTML = '<div class="empty">No hosts yet. Add one to start watching.</div>';
      return;
    }
    return m.shape === 'vector' ? buildVector(m) : buildScalar(m);
  }

  /// Vector metrics: a tile per core, per host.
  ///
  /// Blocks are sized to their core count and packed with flex-wrap, so small
  /// hosts share a row instead of each claiming a full-width strip. Measured
  /// on an 18-host fleet, full-width blocks left single-core hosts occupying
  /// 3% of their row and pushed the grid past the viewport; packing collapses
  /// that to a few rows.
  ///
  /// Tile size stays global. A core must look the same on every host, or
  /// block area reads as importance rather than count - and block *width*
  /// still tracks core count, which is the part worth keeping.
  function buildVector(m) {
    const total = hosts.reduce((a, h) => a + (m.vector(h).length || h.cores || 0), 0);
    if (!total) {
      grid.innerHTML = '<div class="empty">No cores reporting yet.</div>';
      return;
    }

    const GAP = 3;        // between tiles, matches .cores.all
    const CHROME = 28;    // block padding + borders
    const MIN_W = 172;    // enough for hostname, load and the core count

    // Fixed tile size, not scaled to fleet size. A core is a core: at 20px on
    // a big fleet and 52px on a small one, the same load looked like a
    // different quantity depending on how many other machines happened to be
    // on screen. Constant size also makes the packing predictable.
    const px = TILE_PX;
    const avail = Math.max(240, grid.clientWidth - 28);
    // Most tiles that fit across the widest possible block.
    const maxCols = Math.max(1, Math.floor((avail - CHROME + GAP) / (px + GAP)));

    grid.style.setProperty('--tile', px + 'px');
    grid.style.setProperty('--tile-h', Math.max(18, Math.round(px * 0.82)) + 'px');

    // Blocks to draw: a collapsed group is one block holding every member's
    // cores end to end, which is the 'concat' rule made visible. Expanding it
    // swaps in a label and the members' own blocks.
    const items = [];
    {
      const { groups, ungrouped } = TuxAgg.groupHosts(visible());
      for (const g of groups) {
        if (groupOpen(g.name)) {
          items.push({ kind: 'ghead', name: g.name, hosts: g.hosts });
          for (const h of g.hosts) items.push({ kind: 'host', host: h });
        } else {
          items.push({ kind: 'gblock', name: g.name, hosts: g.hosts });
        }
      }
      for (const h of ungrouped) items.push({ kind: 'host', host: h });
    }

    /// Size a block to its core count and fill it with tiles.
    ///
    /// `labelAt` names each tile. In a group block the cores of several hosts
    /// sit side by side, and a hot tile you cannot attribute to a machine is
    /// not information - so the title says which host and which core.
    const layout = (sec, n, labelAt) => {
      const cols = Math.max(1, Math.min(n || 1, maxCols));
      sec.style.setProperty('--cols', cols);
      const natural = cols * px + (cols - 1) * GAP + CHROME;
      if (n >= maxCols) {
        sec.style.flex = '1 1 100%';          // wraps internally to more rows
      } else {
        // Grow to share out a row's leftover width, but not without limit: a
        // block alone on the final row would otherwise stretch across the
        // whole window, putting two tiles in a full-width panel.
        const base = Math.max(MIN_W, natural);
        sec.style.flexBasis = base + 'px';
        sec.style.flexGrow = '1';
        sec.style.maxWidth = Math.round(base * 1.7) + 'px';
      }
      const wrap = sec.querySelector('.hb-body');
      wrap.className = 'hb-body cores all';
      for (let i = 0; i < n; i++) {
        const t = document.createElement('div');
        t.className = 'core';
        t.title = labelAt(i);
        if (px >= 30) t.innerHTML = '<span class="pc"></span>';
        wrap.appendChild(t);
      }
      grid.appendChild(sec);
    };

    items.forEach(it => {
      if (it.kind === 'ghead') {
        const head = document.createElement('div');
        head.className = 'ghead';
        head.dataset.group = it.name;
        const cores = it.hosts.reduce((a, h) => a + (h.cores || 0), 0);
        head.innerHTML = `
          <button class="ftoggle" type="button" aria-expanded="true"
                  title="Collapse ${esc(it.name)}"><span class="chev" aria-hidden="true"></span></button>
          <span class="ghead-name">${esc(it.name)}</span>
          <span class="ghead-meta">${it.hosts.length} hosts · ${cores} cores</span>`;
        head.querySelector('.ftoggle').addEventListener('click', () => toggleGroup(it.name));
        grid.appendChild(head);
        return;
      }

      if (it.kind === 'gblock') {
        const n = it.hosts.reduce((a, h) => a + (m.vector(h).length || h.cores || 0), 0);
        const sec = document.createElement('section');
        sec.className = 'hostblock gblock';
        sec.dataset.group = it.name;
        sec.innerHTML = `
          <header class="hb-head">
            <button class="ftoggle" type="button" aria-expanded="false"
                    title="Show the hosts in ${esc(it.name)}"><span class="chev" aria-hidden="true"></span></button>
            <h2 class="hname">${esc(it.name)}</h2>
            <span class="hb-cpu" data-hb-cpu></span>
            <span class="hb-cores">${it.hosts.length} hosts · ${n} cores</span>
          </header>
          <div class="hb-body"></div>
          <span class="reveal-edge" aria-hidden="true"></span>`;
        sec.querySelector('.ftoggle').addEventListener('click', e => {
          e.stopPropagation();
          toggleGroup(it.name);
        });
        // Flat tile index back to the host that owns it, in the same order
        // the vectors were concatenated.
        const owners = [];
        it.hosts.forEach(mh => {
          const c = m.vector(mh).length || mh.cores || 0;
          for (let i = 0; i < c; i++) owners.push(`${mh.name} core ${i}`);
        });
        layout(sec, n, i => owners[i] || it.name);
        return;
      }

      const h = it.host;
      const n = m.vector(h).length || h.cores || 0;
      const sec = hostBlock(h, n ? `${n} ${n === 1 ? 'core' : 'cores'}` : '? cores');
      layout(sec, n, i => `${h.name} core ${i}`);
    });
  }

  /// Scalar metrics: one comparable bar per host.
  function buildScalar(m) {
    const wrap = document.createElement('div');
    wrap.className = 'fleetbars';

    fleetRows().forEach(r => {
      const row = document.createElement('div');
      if (r.kind === 'group') {
        row.className = 'fbar fgroup';
        row.dataset.group = r.name;
        // The whisker spans the members' range; the fill marks the aggregate.
        // Both are needed - see ADR-008. A bar that shows only the aggregate
        // cannot distinguish a calm group from one tearing itself apart.
        row.innerHTML = `
          <button class="ftoggle" type="button" aria-expanded="${groupOpen(r.name)}"
                  title="Show the hosts in ${esc(r.name)}"><span class="chev" aria-hidden="true"></span></button>
          <span class="fbar-name">${esc(r.name)}</span>
          <span class="fbar-track"><span class="fbar-whisk" hidden></span><span class="fbar-fill"></span></span>
          <span class="fbar-val" data-val></span>
          <span class="fbar-sub" data-sub></span>`;
        row.querySelector('.ftoggle').addEventListener('click', () => toggleGroup(r.name));
      } else {
        row.className = 'fbar' + (r.member ? ' member' : '');
        row.dataset.name = r.host.name;
        row.innerHTML = `
          <span class="dot"></span>
          <span class="fbar-name">${esc(r.host.name)}</span>
          <span class="fbar-track"><span class="fbar-fill"></span></span>
          <span class="fbar-val" data-val></span>
          <span class="fbar-sub" data-sub></span>`;
      }
      wrap.appendChild(row);
    });

    grid.appendChild(wrap);

    // The axis needs saying out loud, with its actual range: a log bar is not
    // a linear one, and a reader who assumes linear misjudges every
    // comparison on screen.
    const note = document.createElement('p');
    note.className = 'scale-note';
    note.dataset.note = '1';
    grid.appendChild(note);
  }

  function hostBlock(h, meta) {
    const sec = document.createElement('section');
    sec.className = 'hostblock';
    sec.dataset.name = h.name;
    sec.innerHTML = `
      <header class="hb-head">
        <span class="dot"></span>
        <h2 class="hname">${esc(h.name)}</h2>
        <span class="hb-cpu" data-hb-cpu></span>
        <span class="hb-temp" data-hb-temp></span>
        <span class="hb-cores">${meta}</span>
      </header>
      <div class="hb-body"></div>
      <span class="reveal-edge" aria-hidden="true"></span>`;
    sec.querySelector('.hb-head').addEventListener('click', () =>
      openHistory({ mode: 'host', host: h.name }));
    sec.querySelector('.hb-head').style.cursor = 'pointer';
    sec.querySelector('.hb-head').title = `History for ${h.name}`;
    return sec;
  }

  function paintFleet() {
    const m = metric();
    if (grid.dataset.shape !== m.shape) { build(); return; }
    return m.shape === 'vector' ? paintVector(m) : paintScalar(m);
  }

  function paintVector(m) {
    // Group blocks hold several hosts' cores end to end, in the order they
    // were concatenated - so they repaint from the same concatenation rather
    // than from any one host.
    grid.querySelectorAll('.gblock').forEach(sec => {
      const name = sec.dataset.group;
      const list = hosts.filter(h => h.group === name);
      const g = TuxAgg.aggregateGroup(m, members(m, list));
      const vals = (g && g.vector) || [];
      const tiles = sec.querySelectorAll('.core');
      if (tiles.length !== vals.length) { build(); return; }

      for (let i = 0; i < tiles.length; i++) {
        const v = vals[i] || 0;
        tiles[i].style.setProperty('--l', normalise(m, v, 100).toFixed(3));
        tiles[i].dataset.band = band(v);
        const pc = tiles[i].firstElementChild;
        if (pc) pc.textContent = Math.round(v);
      }
      // The header states the busiest core in the group, not a mean: the
      // whole reason to look at a core grid is to find the hot one.
      const cpu = sec.querySelector('[data-hb-cpu]');
      if (cpu) cpu.textContent = g && g.value !== null ? m.fmt(g.value) : '—';
    });

    hosts.forEach(h => {
      const sec = grid.querySelector(`.hostblock[data-name="${CSS.escape(h.name)}"]`);
      if (!sec) return;
      const vals = m.vector(h);
      const tiles = sec.querySelectorAll('.core');
      if (tiles.length !== (vals.length || h.cores || 0)) { build(); return; }

      for (let i = 0; i < tiles.length; i++) {
        const v = vals[i] || 0;
        tiles[i].style.setProperty('--l', normalise(m, v, 100).toFixed(3));
        tiles[i].dataset.band = band(v);
        const pc = tiles[i].firstElementChild;
        if (pc) pc.textContent = Math.round(v);
      }
      sec.querySelector('[data-hb-cpu]').textContent = m.fmt(m.scalar(h));
      const ht = sec.querySelector('[data-hb-temp]');
      if (ht) ht.textContent =
        (h.temp === null || h.temp === undefined) ? '' : Math.round(h.temp) + '\u00b0C';
      sec.querySelector('.dot').className = dotClass(h);
    });
  }

  function paintScalar(m) {
    // Peak across the fleet, so every bar shares one axis.
    // Hosts that do not report the metric must not drag the peak to zero.
    // One axis for everything on screen, anchored at whatever is largest -
    // group or host.
    //
    // The spec originally called for separate axes, on the grounds that a
    // group total and a single host are not comparable. Building it proved
    // that wrong in the worst way: dove's 1.1 MB/s bar rendered *longer* than
    // the 2.3 MB/s total of the group it belongs to. A footnote explaining
    // the discrepancy does not repair a picture that is lying.
    //
    // A shared axis is in fact honest here, because a 'sum' aggregate is
    // always at least as large as its biggest member, so lengths stay
    // correctly ordered - longer really does mean more bytes. 'ratio' and
    // 'max' aggregates sit on an absolute scale that ignores the peak
    // entirely. What must not happen is a group being *mistaken* for a host,
    // and that is the row styling's job, not the axis's.
    const aggs = new Map();
    let peak = hosts.reduce((a, h) => Math.max(a, m.scalar(h) ?? 0), 0);
    grid.querySelectorAll('.fgroup').forEach(row => {
      const name = row.dataset.group;
      const list = hosts.filter(h => h.group === name);
      const g = TuxAgg.aggregateGroup(m, members(m, list));
      aggs.set(name, { g, list });
      if (g && g.value !== null) peak = Math.max(peak, g.value);
    });

    aggs.forEach(({ g, list }, name) => {
      const row = grid.querySelector(`.fgroup[data-group="${CSS.escape(name)}"]`);
      if (!row) return;
      const fill = row.querySelector('.fbar-fill');
      const whisk = row.querySelector('.fbar-whisk');
      const dead = !g || g.value === null;

      row.classList.toggle('nodata', dead);
      fill.style.width = dead ? '0%' : (normalise(m, g.value, peak) * 100).toFixed(1) + '%';
      // Severity is the worst member, never the aggregate. A group averaging
      // 40% that contains a host at 97% must not render calm.
      fill.dataset.band =
        m.scale === 'absolute' && !dead ? band(g.severity) : 'cool';

      // The spread. Hidden when a single host reports, where a whisker would
      // draw a range that does not exist.
      if (!dead && g.contributing > 1 && g.max > g.min) {
        const lo = normalise(m, g.min, peak), hi = normalise(m, g.max, peak);
        whisk.hidden = false;
        whisk.style.left = (lo * 100).toFixed(1) + '%';
        whisk.style.width = ((hi - lo) * 100).toFixed(1) + '%';
        whisk.title = `${m.fmt(g.min)} to ${m.fmt(g.max)} across members`;
      } else {
        whisk.hidden = true;
      }

      row.querySelector('[data-val]').textContent = dead ? '—' : m.fmt(g.value);
      row.querySelector('[data-sub]').textContent = groupSub(g, list, m);
      row.classList.toggle('partial', !!(g && g.partial));
    });

    hosts.forEach(h => {
      const row = grid.querySelector(`.fbar[data-name="${CSS.escape(h.name)}"]`);
      if (!row) return;
      const raw = m.scalar(h);
      const missing = raw === null || raw === undefined;
      const v = missing ? 0 : raw;
      const n = missing ? 0 : normalise(m, v, peak);

      const fill = row.querySelector('.fbar-fill');
      fill.style.width = (n * 100).toFixed(1) + '%';
      row.classList.toggle('nodata', missing);
      // Percentage metrics carry the load bands; a rate has no "too hot".
      fill.dataset.band = m.scale === 'absolute' ? band(v) : 'cool';

      row.querySelector('[data-val]').textContent =
        (h.fault || missing) ? '\u2014' : m.fmt(v);
      row.querySelector('[data-sub]').textContent = (!h.fault && m.sub) ? m.sub(h) : '';
      row.querySelector('.dot').className = dotClass(h);
      row.classList.toggle('down', !!h.fault);
    });

    const note = grid.querySelector('[data-note]');
    if (note) {
      if (m.scale === 'log') {
        const { top, bottom } = logWindow(m, peak);
        const d = m.decades || 4;
        note.textContent =
          `Logarithmic axis, ${d} decades: ${m.fmt(bottom)} to ${m.fmt(top)}. ` +
          `Each ${(100 / d).toFixed(0)}% of bar length is a 10x change. ` +
          `Quieter than ${m.fmt(bottom)} reads as empty.`;
      } else {
        note.textContent = 'Linear axis, 0\u2013100%. Directly comparable across hosts.';
      }
      // What a group's bar means differs by metric, and the reader cannot
      // infer it from the picture: 2.3 MB/s is a total, 62°C is the hottest
      // member, 56% is a weighted share. Say which.
      if (grid.querySelector('.fgroup')) {
        const how = { sum: 'the total across its hosts',
                      max: 'its highest host',
                      ratio: 'a share weighted by size',
                      concat: 'every host\u2019s cores together' }[m.agg];
        note.textContent += ` A group row shows ${how}, banded by its worst host.`;
      }
    }
  }

  const dotClass = h =>
    'dot' + (h.fault ? ' warnstate' : (LIVE && !h.seen ? ' pending' : ''));

  function build() {
    $('#histbar').hidden = prefs.view !== 'history' && prefs.view !== 'heat';
    $('#histSwap').hidden = prefs.view === 'heat';
    $('[data-heat-note]').hidden = prefs.view !== 'heat';
    $('#procbar').hidden = prefs.view !== 'procs';
    $('#hostFilterWrap').hidden = prefs.view === 'procs';
    showFilterNote();

    // One place decides which mode class the grid carries. Each builder used to
    // add its own and rely on every *other* builder to strip it, which works
    // only until someone adds a fifth view: heat-mode was added and removed
    // nowhere, so after one visit to Heat the grid kept `display:block` and
    // every card in every other view went full-width. An exhaustive toggle
    // cannot rot that way.
    for (const [v, c] of Object.entries(MODE_CLASS)) {
      grid.classList.toggle(c, prefs.view === v);
    }

    if (prefs.view !== 'procs') { stopProcs(); }
    if (prefs.view === 'history') return buildHistory();
    stopHistoryTimer();
    if (prefs.view === 'procs') return buildProcs();
    if (prefs.view === 'heat') return buildHeat();
    if (prefs.view === 'all') return buildFleet();
    document.body.dataset.bands = 'on';
    grid.style.removeProperty('--tile');

    // build() tears down the DOM, so remember which card was open. Without
    // this, adding a host or a core-count change silently collapses the
    // detail panel the user was reading.
    const wasOpen = grid.querySelector('.card.expanded')?.dataset.name;
    grid.innerHTML = '';
    if (!hosts.length) {
      grid.innerHTML = '<div class="empty">No hosts. Add one to start watching.</div>';
      return;
    }
    visible().forEach(h => {
      const el = document.createElement('article');
      el.className = 'card';
      el.dataset.id = h.id;
      el.dataset.name = h.name;
      el.innerHTML = `
        <div class="chead">
          <span class="grip" aria-hidden="true" title="Drag to reorder">
            <svg width="9" height="13" viewBox="0 0 9 13"><g fill="currentColor">
              <circle cx="1.6" cy="1.6" r="1.3"/><circle cx="7.4" cy="1.6" r="1.3"/>
              <circle cx="1.6" cy="6.5" r="1.3"/><circle cx="7.4" cy="6.5" r="1.3"/>
              <circle cx="1.6" cy="11.4" r="1.3"/><circle cx="7.4" cy="11.4" r="1.3"/>
            </g></svg>
          </span>
          <span class="dot"></span>
          <h2 class="hname">${h.name}</h2>
          <span class="chip" data-tag>${h.distro || '&mdash;'}</span>
          <span class="chip chip-virt" data-virt hidden></span>
          <span class="chip chip-up" data-uptime hidden></span>
          <span class="chip" data-cores>${h.cores || '?'}c</span>
          <span class="tb-grow"></span>
          <button class="kill" title="Remove ${h.name}" aria-label="Remove ${h.name}">
            <svg width="11" height="11" viewBox="0 0 10 10"><path d="M0 0l10 10M10 0L0 10" stroke="currentColor" stroke-width="1.4"/></svg>
          </button>
        </div>
        <div class="fault" data-fault hidden>
          <b data-fault-title></b>
          <span data-fault-detail></span>
        </div>
        <div class="cpurow">
          <div class="readout">
            <div class="num" data-cpu>0<span>%</span></div>
            <div class="cap">CPU</div>
          </div>
          <canvas class="spark"></canvas>
        </div>
        <div class="cores mini"></div>
        <div class="metrics">
          <div class="m"><span class="k">RAM</span><span class="v" data-ram></span></div>
          <div class="m"><span class="k">I/O</span><span class="v" data-dio></span></div>
          <div class="m"><span class="k">Net</span><span class="v" data-net></span></div>
          <div class="m" data-fs-chip hidden><span class="k">Disk</span><span class="v" data-fs></span></div>
          <div class="m" data-temp-chip><span class="k">Temp</span><span class="v" data-temp></span></div>
          <div class="m" data-steal-chip hidden><span class="k">Steal</span><span class="v" data-steal></span></div>
          <div class="m" data-gpu-chip hidden><span class="k" data-gpu-k>GPU</span><span class="v" data-gpu></span></div>
          ${h.gpu ? '<div class="m"><span class="k">GPU</span><span class="v" data-gpu></span></div>' : ''}
        </div>
        <div class="card-actions">
          <button class="card-toggle" aria-expanded="false">
            <svg width="10" height="10" viewBox="0 0 10 10"><path d="M1 3.5L5 7l4-3.5" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round"/></svg>
            <span class="tlabel">Per-core detail</span>
          </button>
          <button class="card-toggle card-hist" data-hist>History</button>
        </div>
        <div class="detail">
          <div>
            <p class="dhead">${h.cores} logical cores</p>
            <div class="cores full"></div>
          </div>
          <div class="charts">
            <div class="mini"><div class="top"><span class="lbl">Memory</span><span class="val" data-d-ram></span></div><canvas data-c="ram"></canvas></div>
            <div class="mini"><div class="top"><span class="lbl">Disk I/O</span><span class="val" data-d-dio></span></div><canvas data-c="dio"></canvas></div>
            <div class="mini"><div class="top"><span class="lbl">Network</span><span class="val" data-d-net></span></div><canvas data-c="net"></canvas></div>
            ${h.gpu ? `<div class="mini"><div class="top"><span class="lbl">${h.gpu}</span><span class="val" data-d-gpu></span></div><canvas data-c="gpu"></canvas></div>` : ''}
          </div>
        </div>`;

      const mini = el.querySelector('.cores.mini');
      const full = el.querySelector('.cores.full');
      for (let i = 0; i < h.cores; i++) {
        const t = document.createElement('div');
        t.className = 'core';
        mini.appendChild(t);
        const f = document.createElement('div');
        f.className = 'core';
        f.innerHTML = `<span class="idx">${i}</span><span class="pc">0</span>`;
        full.appendChild(f);
      }

      // Removal is handled by a delegated listener on the grid, registered
      // once per mode. Live mode must round-trip through the backend so the
      // sampler task is actually stopped; removing from the array here would
      // make the card vanish and then reappear on the next sample.
      el.querySelector('.card-toggle').addEventListener('click', () => {
        const open = el.classList.contains('expanded');
        grid.querySelectorAll('.card.expanded').forEach(c => {
          c.classList.remove('expanded');
          c.querySelector('.card-toggle').setAttribute('aria-expanded', 'false');
          c.querySelector('.tlabel').textContent = 'Per-core detail';
        });
        if (!open) {
          el.classList.add('expanded');
          el.querySelector('.card-toggle').setAttribute('aria-expanded', 'true');
          el.querySelector('.tlabel').textContent = 'Collapse';
        }
        paint();
      });
      // Entering history from a host carries that host with it.
      el.querySelector('[data-hist]').addEventListener('click', ev => {
        ev.stopPropagation();
        openHistory({ mode: 'host', host: h.name });
      });

      // Drag to reorder. Only from the header, so dragging never fights the
      // expand toggle or the remove button.
      const head = el.querySelector('.chead');
      head.draggable = true;
      head.addEventListener('dragstart', ev => {
        // A rebuild mid-drag would yank the element out from under the
        // pointer, so repaints are suspended until the drag ends.
        dragging = h.name;
        frozen = true;
        el.classList.add('is-dragging');
        ev.dataTransfer.effectAllowed = 'move';
        ev.dataTransfer.setData('text/plain', h.name);
      });
      head.addEventListener('dragend', () => {
        dragging = null;
        frozen = false;
        grid.querySelectorAll('.card').forEach(c =>
          c.classList.remove('is-dragging', 'drop-before', 'drop-after'));
      });

      el.addEventListener('dragover', ev => {
        if (!dragging || dragging === h.name) return;
        ev.preventDefault();
        const r = el.getBoundingClientRect();
        const after = ev.clientX > r.left + r.width / 2;
        el.classList.toggle('drop-after', after);
        el.classList.toggle('drop-before', !after);
      });
      el.addEventListener('dragleave', () =>
        el.classList.remove('drop-before', 'drop-after'));
      el.addEventListener('drop', ev => {
        ev.preventDefault();
        const after = el.classList.contains('drop-after');
        el.classList.remove('drop-before', 'drop-after');
        commitOrder(dragging, h.name, after);
      });

      el.appendChild(Object.assign(document.createElement('span'),
        { className: 'reveal-edge', ariaHidden: 'true' }));

      grid.appendChild(el);
    });

    if (wasOpen) {
      const el = grid.querySelector(`.card[data-name="${CSS.escape(wasOpen)}"]`);
      if (el) {
        el.classList.add('expanded');
        el.querySelector('.card-toggle').setAttribute('aria-expanded', 'true');
        el.querySelector('.tlabel').textContent = 'Collapse';
      }
    }
  }

  // ---- paint values ----
  function paint() {
    if (frozen) return;   // a rebuild mid-drag would drop the dragged card
    // History redraws on its own cadence, not on every 1 Hz sample: refetching
    // a week of buckets once a second would be absurd.
    if (prefs.view === 'history' || prefs.view === 'procs' || prefs.view === 'heat') return;
    if (prefs.view === 'all') return paintFleet();
    const A = css('--accent'), MEM = css('--viz-mem'),
          DSK = css('--viz-disk'), NET = css('--viz-net'), GPU = css('--viz-gpu');
    hosts.forEach(h => {
      const el = grid.querySelector(`.card[data-id="${h.id}"]`);
      if (!el) return;
      const cpu = h.hist.cpu[h.hist.cpu.length - 1] || 0;

      // A fault says what went wrong instead of blanking the card. The last
      // known numbers stay on screen, dimmed, because "it was at 90% when it
      // died" is usually the most useful thing on the card.
      const fbox = el.querySelector('[data-fault]');
      if (h.fault) {
        el.classList.add('down');
        fbox.hidden = false;
        el.querySelector('[data-fault-title]').textContent = faultTitle(h.fault);
        el.querySelector('[data-fault-detail]').textContent = faultDetail(h.fault);
        el.querySelector('.dot').className = 'dot warnstate';
      } else {
        el.classList.remove('down');
        fbox.hidden = true;
      }

      if (h.load) el.querySelector('[data-tag]').textContent =
        'load ' + h.load[0].toFixed(2);

      const up = el.querySelector('[data-uptime]');
      if (up) {
        up.hidden = h.uptime == null;
        if (h.uptime != null) up.textContent = 'up ' + humanUptime(h.uptime);
      }
      // What this machine actually is, on the card that names it.
      if (h.facts) {
        const f = h.facts;
        el.querySelector('.hname').title =
          [f.cpu_model, f.os, f.kernel].filter(Boolean).join('\n');
      }
      // What kind of machine this is. Bare metal gets no badge - it is what a
      // reader already assumes, and a chip on all eighteen cards would bury
      // the three that are not.
      const vchip = el.querySelector('[data-virt]');
      if (vchip) {
        const label = machineLabel(h);
        vchip.hidden = !label;
        vchip.textContent = label;
        vchip.dataset.kind = machine(h);
        vchip.title = machine(h) === 'container'
          ? 'Shares its host\u2019s kernel: uptime and core count are the host\u2019s, and memory is a limit rather than hardware'
          : 'A guest. Its cores are vCPUs and its memory is an allocation \u2014 both are honest about this guest, not about the machine it runs on';
      }
      el.querySelector('[data-cores]').textContent = (h.cores || '?') + 'c';

      el.querySelector('[data-cpu]').innerHTML = Math.round(cpu) + '<span>%</span>';
      if (!h.fault) {
        // A host that has never reported is connecting, not up. Showing it
        // green would claim a fact we do not have yet.
        const cls = LIVE && !h.seen ? ' pending' : (cpu > 85 ? ' warnstate' : '');
        el.querySelector('.dot').className = 'dot' + cls;
      }
      el.querySelector('[data-ram]').textContent = `${gb(h.ram)} / ${gb(h.ramGB)} GB`;
      el.querySelector('[data-dio]').textContent = bps(h.dio);
      el.querySelector('[data-net]').textContent = bps(h.net);

      // A host with no CPU sensor - normal on a VM - shows a dash rather than
      // a zero, which would read as an implausibly cold CPU. Shown rather than
      // hidden: dropping the chip changed the chip count, so cards with a
      // sensor wrapped to two lines and cards without stayed on one, giving
      // rows visibly different heights. A dash also states the absence
      // explicitly instead of leaving it to be inferred.
      // Unlike temperature, the GPU chip is hidden outright when a host has
      // no card. A dash would imply a GPU that failed to report, when the
      // truth is there is no GPU - and unlike temperature this does not
      // change the chip count unevenly, because whole classes of host have
      // one or none.
      const gchip = el.querySelector('[data-gpu-chip]');
      if (gchip) {
        gchip.hidden = !h.gpu;
        if (h.gpu) {
          el.querySelector('[data-gpu-k]').textContent = shortGpu(h.gpu);
          el.querySelector('[data-gpu]').textContent =
            `${Math.round(h.gpuU)}% ${gb(h.gpuUsed / 1024)}/${gb(h.gpuTotal / 1024)}G`;
        }
      }

      // Disk capacity, hidden until a df frame has arrived - which takes a
      // few seconds, and a 0% would be a lie in the meantime.
      const fchip = el.querySelector('[data-fs-chip]');
      if (fchip) {
        const p = fsPct(h);
        fchip.hidden = p === null;
        if (p !== null) {
          const f = fullestFs(h);
          const v = fchip.querySelector('[data-fs]');
          v.textContent = `${Math.round(p)}%`;
          v.dataset.band = band(p);
          fchip.title = `${f.mount} - ${gb(f.used_kb / 1048576)} of ${gb(f.total_kb / 1048576)} GB used`;
        }
      }

      // Steal: time the hypervisor gave to somebody else.
      //
      // Shown only on guests, and only when non-zero. On bare metal it is
      // structurally zero - there is no hypervisor to take the time - so a
      // figure there would imply a measurement that does not exist. On a
      // guest it answers "why is this slow when it looks idle", which is the
      // question a guest actually raises, and the one that pairs a guest card
      // with its hypervisor's.
      const schip = el.querySelector('[data-steal-chip]');
      if (schip) {
        const st = h.breakdown ? h.breakdown.steal : null;
        const show = stealIsMeaningful(h) && typeof st === 'number' && st >= 0.5;
        schip.hidden = !show;
        if (show) {
          const v = el.querySelector('[data-steal]');
          v.textContent = st.toFixed(1) + '%';
          v.dataset.band = band(st * 4);   // 25% steal is already severe
          schip.title = 'Time this guest was ready to run but the hypervisor '
            + 'gave the CPU to someone else. Not this machine being busy \u2014 '
            + 'this machine being kept waiting.';
        }
      }

      // The full split, on the number it explains. "Not busy, waiting on
      // disk" is a different problem with a different fix from "busy".
      if (h.breakdown) {
        const b = h.breakdown;
        const parts = [`user ${b.user.toFixed(1)}%`, `system ${b.system.toFixed(1)}%`,
                       `io wait ${b.iowait.toFixed(1)}%`];
        if (stealIsMeaningful(h)) parts.push(`steal ${b.steal.toFixed(1)}%`);
        const cpuEl = el.querySelector('[data-cpu]');
        if (cpuEl) cpuEl.title = parts.join(' \u00b7 ');
      }

      const tchip = el.querySelector('[data-temp-chip]');
      if (tchip) {
        const hasTemp = h.temp !== null && h.temp !== undefined;
        tchip.classList.toggle('nodata', !hasTemp);
        el.querySelector('[data-temp]').textContent =
          hasTemp ? Math.round(h.temp) + '\u00b0C' : '\u2014';
        // The chip shows the CPU, which is the reading the ranking vouches
        // for. Every other sensor is here, because the hottest thing in the
        // box is often an NVMe and the chip must not be read as the maximum.
        tchip.title = (h.temps && h.temps.length)
          ? 'All sensors\n' + h.temps
              .map(t => `${t.name}  ${Math.round(t.celsius)}\u00b0C`).join('\n')
          : 'No temperature sensors on this host';
      }
      if (h.gpu) el.querySelector('[data-gpu]').textContent = Math.round(h.gpuU) + '%';

      const minis = el.querySelectorAll('.cores.mini .core');
      const fulls = el.querySelectorAll('.cores.full .core');
      for (let i = 0; i < h.cores; i++) {
        const v = h.core[i], b = band(v);
        minis[i].style.setProperty('--l', (v / 100).toFixed(3));
        minis[i].dataset.band = b;
        if (el.classList.contains('expanded')) {
          fulls[i].style.setProperty('--l', (v / 100).toFixed(3));
          fulls[i].dataset.band = b;
          fulls[i].querySelector('.pc').textContent = Math.round(v);
        }
      }

      draw(el.querySelector('.spark'), h.hist.cpu, A, 100);
      if (el.classList.contains('expanded')) {
        draw(el.querySelector('[data-c="ram"]'), h.hist.ram, MEM, 100);
        draw(el.querySelector('[data-c="dio"]'), h.hist.dio, DSK, 0);
        draw(el.querySelector('[data-c="net"]'), h.hist.net, NET, 0);
        el.querySelector('[data-d-ram]').textContent = gb(h.ram) + ' GB';
        el.querySelector('[data-d-dio]').textContent = bps(h.dio);
        el.querySelector('[data-d-net]').textContent = bps(h.net);
        if (h.gpu) {
          draw(el.querySelector('[data-c="gpu"]'), h.hist.gpu, GPU, 100);
          el.querySelector('[data-d-gpu]').textContent = Math.round(h.gpuU) + '%';
        }
      }
    });
  }

  function tally() {
    refreshMetricOptions();
    $('#nhosts').textContent = hosts.length;
    $('#nup').textContent = hosts.filter(h => !h.fault && (!LIVE || h.seen)).length;

    // Physical and virtual cores are stated separately, never summed.
    //
    // A guest's vCPUs are carved out of a host that may be in this very fleet
    // - athens' cores come from coot's - so adding them counts the same
    // silicon twice and reports a machine that does not exist. Splitting the
    // figure needs no knowledge of which host owns which guest, only of what
    // each host is, so it cannot be wrong the way a guessed parent could.
    let phys = 0, virt = 0;
    for (const h of hosts) {
      const c = h.cores || 0;
      if (machine(h) === 'metal') phys += c; else virt += c;
    }
    const el = $('#ncores');
    // "112 + 52 cores", not "112 + 52 cores (physical + virtual)". The label
    // explaining the split cost about 150px of a toolbar that has to hold a
    // host select and a filter as well, and the sum being split is itself the
    // signal - the words belong in the tooltip, where they are one hover away
    // rather than permanently in the way.
    el.textContent = virt && phys ? `${phys} + ${virt}` : String(phys + virt);
    el.title = virt && phys
      ? `${phys} physical cores, plus ${virt} vCPUs in guests — not added ` +
        `together, because a guest's cores come out of a host's`
      : '';
  }

  // Fault text. Each variant names a different fix, which is the entire
  // reason HostFault is not collapsed into a single "offline" state.
  function faultTitle(f) {
    return {
      auth_failed: 'Authentication failed',
      unreachable: 'Host unreachable',
      sampler_failed: 'Sampler failed',
      stalled: 'Connection stalled',
    }[f.kind] || 'Disconnected';
  }

  function faultDetail(f) {
    if (f.kind === 'stalled') return `no data for ${f.detail?.since_secs ?? '?'}s`;
    if (f.kind === 'auth_failed') return `${f.detail || ''} - check \`ssh-add -l\` and that \`ssh ${f.host}\` works`;
    if (f.kind === 'unreachable') return `${f.detail || ''} - check the address, network or VPN`;
    return f.detail || 'no detail reported';
  }

  // ---- clock / data source -------------------------------------------
  let rate = 1000, timer = null;
  const tick = () => { hosts.forEach(step); paint(); };
  const start = () => { clearInterval(timer); timer = setInterval(tick, rate); };

  $('#themeBtn').addEventListener('click', () => {
    const r = document.documentElement;
    const dark = r.getAttribute('data-theme') === 'dark' ||
      (!r.getAttribute('data-theme') && matchMedia('(prefers-color-scheme: dark)').matches);
    r.setAttribute('data-theme', dark ? 'light' : 'dark');
    paint();
  });

  // ---- view + sort controls ----------------------------------------------
  // Populate the metric picker from the registry, so a new metric needs no
  // markup change.
  const msel = $('#metricSel');

  /** Which pref the shared metric picker reads and writes in this view. */
  const metricPref = () => (prefs.view === 'heat' ? 'heatMetric' : 'metric');

  function refreshMetricOptions() {
    // Heat draws one row per host, so a vector metric has nothing to put in
    // it - the same reason History's compare-across-fleet mode excludes them.
    const heat = prefs.view === 'heat';
    const avail = availableMetrics().filter(([, m]) => !heat || m.shape !== 'vector');
    const ids = avail.map(([id]) => id);
    const current = (heat ? 'heat|' : 'all|') + ids.join(',');
    if (msel.dataset.ids === current) return;
    msel.dataset.ids = current;

    msel.innerHTML = '';
    for (const [id, m] of avail) {
      const o = document.createElement('option');
      o.value = id; o.textContent = m.label;
      msel.appendChild(o);
    }
    // A metric can vanish when its last reporting host goes away. Only then
    // is rewriting the preference correct - never merely because data has not
    // arrived yet.
    const reporting = hosts.some(h => h.seen || !LIVE);
    const key = metricPref();
    if (reporting && !ids.includes(prefs[key])) {
      prefs[key] = ids[0] || (heat ? 'cpu' : 'cores');
      savePrefs();
    }
    msel.value = prefs[key];
  }
  refreshMetricOptions();

  function setView(v) {
    prefs.view = v; savePrefs();
    $('#viewHosts').setAttribute('aria-pressed', String(v === 'hosts'));
    $('#viewAll').setAttribute('aria-pressed', String(v === 'all'));
    $('#viewHist').setAttribute('aria-pressed', String(v === 'history'));
    $('#viewHeat').setAttribute('aria-pressed', String(v === 'heat'));
    $('#viewProcs').setAttribute('aria-pressed', String(v === 'procs'));
    // Heat shares the Fleet view's metric picker, reading its own pref.
    $('#metricWrap').hidden = v !== 'all' && v !== 'heat';
    $('#histWrap').hidden = v !== 'history';
    // Card ordering means nothing to a process table, which sorts itself.
    $('#sortSel').closest('.sortwrap').hidden = v === 'procs';
    build(); paint();
  }

  msel.addEventListener('change', e => {
    prefs[metricPref()] = e.target.value; savePrefs();
    build(); paint();
  });
  $('#viewHosts').addEventListener('click', () => setView('hosts'));
  $('#viewAll').addEventListener('click', () => setView('all'));
  // The tab is contextual too: coming from the Fleet view means "this metric,
  // everywhere", which is the slice already on screen. From Hosts it keeps
  // whatever host was last shown.
  $('#viewProcs').addEventListener('click', () => setView('procs'));
  $('#viewHeat').addEventListener('click', () => {
    // Arriving from Fleet with a scalar metric on screen, keep it: "this
    // metric, everywhere" gains a time axis rather than changing subject.
    const m = METRICS[prefs.metric];
    if (prefs.view === 'all' && m && m.shape !== 'vector') {
      prefs.heatMetric = prefs.metric;
      savePrefs();
    }
    setView('heat');
  });
  $('#viewHist').addEventListener('click', () => {
    if (prefs.view === 'all') {
      // Vector metrics are valid in history now - "CPU cores across the
      // fleet" is every host's small multiples - so the metric on screen
      // carries over unchanged rather than being downgraded to CPU.
      prefs.slice = { mode: 'metric', metric: prefs.metric };
      savePrefs();
    }
    setView('history');
  });

  $('#sortSel').value = prefs.sort;
  $('#sortSel').addEventListener('change', e => {
    prefs.sort = e.target.value; savePrefs();
    build(); paint();
  });

  // Restore the remembered view before the first render.
  $('#viewHosts').setAttribute('aria-pressed', String(prefs.view === 'hosts'));
  $('#viewAll').setAttribute('aria-pressed', String(prefs.view === 'all'));
  $('#viewHist').setAttribute('aria-pressed', String(prefs.view === 'history'));
  $('#viewProcs').setAttribute('aria-pressed', String(prefs.view === 'procs'));
  $('#viewHeat').setAttribute('aria-pressed', String(prefs.view === 'heat'));
  // The filter belongs to the views that draw hosts. Processes has its own,
  // which searches different things.
  $('#hostFilterWrap').hidden = prefs.view === 'procs';
  $('#metricWrap').hidden = prefs.view !== 'all' && prefs.view !== 'heat';
  $('#histWrap').hidden = prefs.view !== 'history';
  $('#sortSel').closest('.sortwrap').hidden = prefs.view === 'procs';
  $('#histbar').hidden = prefs.view !== 'history' && prefs.view !== 'heat';

  /// How many columns currently fit, for each of the two grid shapes.
  ///
  /// The layout key, not the width: rebuilding on every pixel would thrash,
  /// and rebuilding on width alone oscillates when a scrollbar appears and
  /// disappears across the threshold. What actually matters is whether the
  /// number of columns changed.
  function layoutKey() {
    const avail = Math.max(240, grid.clientWidth - 28);
    const coreCols = Math.max(1, Math.floor((avail - 26 + 5) / (CORE_CHART_W + 5)));
    const tileCols = Math.max(1, Math.floor((avail - 28 + 3) / (TILE_PX + 3)));
    return `${coreCols}:${tileCols}`;
  }

  let laidOutAt = '';

  /// Rebuild the width-sensitive views when the space they measured changes.
  ///
  /// Both grids size themselves from the container at build time, so a build
  /// that runs before the window has settled lays out against the wrong
  /// width. On first launch History came up four charts to a row and only
  /// corrected itself when switching tabs happened to rebuild it - the
  /// resize listener this replaces covered the Fleet view alone, and nothing
  /// covered the initial paint at all.
  function relayout() {
    const key = layoutKey();
    if (key === laidOutAt) return;
    laidOutAt = key;
    if (prefs.view === 'all' || prefs.view === 'history') build();
  }

  // A ResizeObserver rather than a window resize handler: it also fires for
  // the first real layout, which is the case that was broken, and for a
  // container that changes width without the window doing so.
  if (typeof ResizeObserver === 'function') {
    new ResizeObserver(() => requestAnimationFrame(relayout)).observe(grid);
  } else {
    addEventListener('resize', relayout);
  }

  // ---- history -----------------------------------------------------------
  //
  // History has no default slice: it inherits one from wherever it was
  // entered. The user is already looking at a host or a metric when they ask
  // for its history, so re-asking would be a question the app can answer.

  /// Enter history showing `slice`, remembering it for a direct return.
  function openHistory(slice) {
    if (slice) { prefs.slice = slice; savePrefs(); }
    setView('history');
  }

  /// A metric that makes sense as one line per host.
  ///
  /// Vector metrics have no such line - "CPU cores" across the fleet is a
  /// grid, not a chart - so any path into metric mode falls back to CPU.
  function scalarMetricId(pref) {
    const m = METRICS[pref];
    return m && m.shape !== 'vector' ? pref : 'cpu';
  }

  function currentSlice() {
    const s = prefs.slice;
    if (s && s.mode === 'host' && hosts.some(h => h.name === s.host)) return s;
    if (s && s.mode === 'metric' && METRICS[s.metric]) return s;
    // Nothing remembered or the subject is gone: fall back to what is on
    // screen rather than an arbitrary choice.
    return hosts.length
      ? { mode: 'host', host: ordered()[0].name }
      : { mode: 'metric', metric: prefs.metric };
  }

  function buildHistory() {
    startHistoryTimer();
    grid.innerHTML = '';
    // The chart grid does its own column layout, so the outer grid must give
    // it the full width. Left as a multi-column grid it becomes a single
    // 320px item and every chart stacks inside that one track.
    grid.style.removeProperty('--tile');
    $('#histbar').hidden = false;

    if (!hosts.length) {
      grid.innerHTML = '<div class="empty">No hosts yet. Add one to start watching.</div>';
      return;
    }

    const slice = currentSlice();
    const wrap = document.createElement('div');
    wrap.className = 'charts-grid';

    const sub = $('#histSubject');

    if (slice.mode === 'host') {
      // Pick a different host without leaving History.
      $('[data-hist-label]').textContent = 'Host';
      sub.innerHTML = visible()
        .map(h => `<option value="${esc(h.name)}"${h.name === slice.host ? ' selected' : ''}>${esc(h.name)}</option>`)
        .join('');
      $('#histSwap').textContent = 'Compare across fleet';
      for (const [id, m] of availableMetrics()) {
        if (m.shape === 'vector') continue;
        wrap.appendChild(chartEl(`${slice.host}::${id}`, m.label, id, slice.host));
      }
      // One chart per sensor. These are not registry metrics - the set differs
      // per host, and a fleet-wide list would offer an NVMe that only one box
      // has. An NVMe warming up over an hour is the shape worth seeing, and it
      // was collected and discarded until now.
      const sh = hosts.find(x => x.name === slice.host);
      for (const t of (sh && sh.temps) || []) {
        wrap.appendChild(chartEl(`${slice.host}::${sensorMetric(t)}`,
                                 t.name, sensorMetric(t), slice.host));
      }
    } else {
      // Pick a different metric without leaving History. Vector metrics are
      // excluded: there is no single line per host to compare.
      $('[data-hist-label]').textContent = 'Metric';
      sub.innerHTML = availableMetrics()
        .map(([id, m]) => `<option value="${id}"${id === slice.metric ? ' selected' : ''}>${esc(m.label)}</option>`)
        .join('');
      $('#histSwap').textContent = 'Show one host';

      const vm = METRICS[slice.metric];
      if (vm && vm.shape === 'vector') {
        // A vector metric across the fleet is every host's small multiples,
        // grouped by host. There is no single line to overlay.
        const pack = document.createElement('div');
        pack.className = 'cores-pack';
        for (const h of visible()) {
          if (h.cores > 0) pack.appendChild(coreChartsEl(h, true));
        }
        grid.appendChild(pack);
        refreshHistory();
        return;
      }
      // Groups first, then every host. The group charts are added rather
      // than substituted: a summary that costs you the ability to see which
      // member caused a spike is a worse trade than a longer page.
      const { groups } = TuxAgg.groupHosts(visible());
      for (const g of groups) {
        const el = chartEl(`group:${g.name}::${slice.metric}`,
                           `${g.name} · ${g.hosts.length} hosts`, slice.metric, '');
        el.classList.add('group-chart');
        el.dataset.group = g.name;
        wrap.appendChild(el);
      }
      for (const h of visible()) {
        wrap.appendChild(chartEl(`${h.name}::${slice.metric}`, h.name, slice.metric, h.name));
      }
    }

    grid.appendChild(wrap);

    // Per-core small multiples, the Task Manager shape. Only in host mode:
    // across the fleet there is no sensible way to line up core 7 of one box
    // with core 7 of another.
    if (slice.mode === 'host') {
      const h = hosts.find(x => x.name === slice.host);
      if (h && h.cores > 0) grid.appendChild(coreChartsEl(h));
    }

    refreshHistory();
  }

  /// A metric definition for a chart, including per-sensor series that are
  /// not registry entries. They all render as degrees on the same 0-100 scale
  /// as the CPU, so an NVMe chart is directly comparable to it.
  function metricFor(id) {
    if (METRICS[id]) return METRICS[id];
    if (String(id).startsWith('temp.')) return METRICS.temp;
    return null;
  }

  function chartEl(key, label, metric, host) {
    const el = document.createElement('div');
    el.className = 'chart';
    el.dataset.key = key;
    el.dataset.metric = metric;
    el.dataset.host = host;
    el.innerHTML = `
      <div class="chart-head">
        <span class="chart-name">${esc(label)}</span>
        <span class="chart-peak" data-peak></span>
        <span class="chart-latest" data-latest></span>
      </div>
      <canvas></canvas>
      <span class="reveal-edge" aria-hidden="true"></span>`;
    return el;
  }

  /// The per-core grid: one small chart per core, all the same size.
  /// Fixed chart size, same on every host - the same rule as the fleet
  /// tiles, and for the same reason: a core on a 2-core box must not look
  /// bigger than a core on a 32-core box.
  const CORE_CHART_W = 126;

  function coreChartsEl(h, packed) {
    const sec = document.createElement('section');
    sec.className = 'cores-hist' + (packed ? ' packed' : '');
    sec.dataset.host = h.name;
    sec.innerHTML = `
      <div class="cores-hist-head">
        <span class="ch-host">${esc(h.name)}</span>
        <span class="ch-count">${h.cores}c</span>
        <span class="chart-peak" data-cores-peak></span>
      </div>
      <div class="core-charts"></div>
      <span class="reveal-edge" aria-hidden="true"></span>`;

    const wrap = sec.querySelector('.core-charts');
    if (packed) {
      // Width tracks core count, so blocks pack and a 2-core host stops
      // claiming a full-width strip with two charts in it.
      const GAP = 5, CHROME = 26;
      const avail = Math.max(240, grid.clientWidth - 28);
      const maxCols = Math.max(1, Math.floor((avail - CHROME + GAP) / (CORE_CHART_W + GAP)));
      const cols = niceCols(h.cores, maxCols);
      wrap.style.gridTemplateColumns = `repeat(${cols}, ${CORE_CHART_W}px)`;
      const natural = cols * CORE_CHART_W + (cols - 1) * GAP + CHROME;
      if (h.cores >= maxCols) {
        sec.style.flex = '1 1 100%';
      } else {
        sec.style.flexBasis = natural + 'px';
        sec.style.flexGrow = '1';
        sec.style.maxWidth = Math.round(natural * 1.6) + 'px';
      }
    }
    if (!packed) {
      // Host mode stretches its charts to fill, but the column *count* should
      // still snap - auto-fill picks the same arbitrary 9 the packed layout
      // used to.
      const GAP = 5, MIN_W = 132;
      const avail = Math.max(240, grid.clientWidth - 40);
      const maxCols = Math.max(1, Math.floor((avail + GAP) / (MIN_W + GAP)));
      wrap.style.gridTemplateColumns = `repeat(${niceCols(h.cores, maxCols)}, minmax(0, 1fr))`;
    }

    for (let i = 0; i < h.cores; i++) {
      const el = document.createElement('div');
      el.className = 'core-chart';
      el.dataset.core = i;
      el.innerHTML = `<span class="idx">${i}</span><span class="pk"></span><canvas></canvas>`;
      wrap.appendChild(el);
    }
    return sec;
  }

  /// Draw the per-core grid, fetching every core in one call.
  async function refreshCoreCharts(secs, win) {
    await Promise.all(
      [...grid.querySelectorAll('.cores-hist')]
        .map(sec => refreshOneCoreGrid(sec, secs, win)));
  }

  async function refreshOneCoreGrid(sec, secs, win) {
    if (!sec) return;
    const host = sec.dataset.host;
    const cells = [...sec.querySelectorAll('.core-chart')];
    if (!cells.length) return;

    const metrics = cells.map(c => `core.${c.dataset.core}`);
    const budget = Math.max(40, Math.round(cells[0].querySelector('canvas').clientWidth || 130));

    let data = {};
    if (LIVE) {
      try {
        data = await TAURI.core.invoke('query_history_many', {
          host, metrics, fromSecsAgo: secs, toSecsAgo: 0, maxPoints: budget,
        });
      } catch { data = {}; }
    } else {
      metrics.forEach(m => { data[m] = simHistory(host, 'cpu', secs, budget); });
    }

    let fleetPeak = 0;
    for (const c of cells) {
      const pts = data[`core.${c.dataset.core}`] || [];
      drawHistory(c.querySelector('canvas'), pts, METRICS.cpu, win);
      const pk = pts.reduce((a, p) => Math.max(a, p.max), 0);
      fleetPeak = Math.max(fleetPeak, pk);
      c.querySelector('.pk').textContent = pts.length ? Math.round(pk) + '%' : '';
    }
    const head = sec.querySelector('[data-cores-peak]');
    if (head) head.textContent = fleetPeak ? `busiest core peaked at ${Math.round(fleetPeak)}%` : '';
  }

  /// Fetch and draw every visible chart for the current window.
  ///
  /// One window across all of them: scrubbing one moves them all, which is
  /// the point when correlating a spike - seeing that the CPU jump and the
  /// disk jump are the same second is the question being asked.
  // History refreshes on a timer rather than on every 1 Hz sample: refetching
  // a week of buckets once a second would be absurd, and 148 core charts even
  // more so. Two seconds keeps it visibly live without that.
  const HIST_REFRESH_MS = 2000;
  let histTimer = null;
  let histBusy = false;

  // ---------------------------------------------------------------- HEAT
  //
  // The fleet as rows, time as columns. Neither of the other views can show
  // this: the live grid is every host at one instant, History is a window but
  // only a few subjects. Here a whole window and the whole fleet are on screen
  // together, which is what makes "coot spiked twenty minutes ago" a thing you
  // notice rather than a thing you go looking for.
  //
  // Every host keeps its own row. This is the one view with room for all
  // nineteen, so nothing is aggregated and ADR-008 has nothing to hide behind:
  // groups are drawn as headings, not as summaries.

  const heatMetric = () => METRICS[prefs.heatMetric] || METRICS.cpu;

  function buildHeat() {
    stopHistoryTimer();
    document.body.dataset.bands = 'on';
    grid.style.removeProperty('--tile');
    refreshMetricOptions();

    const list = heatOrder(visible());
    if (!list.length) { grid.innerHTML = ''; return showEmpty('No hosts match the filter.'); }

    const breaks = new Set(groupBreaks(list));
    let html = '<div class="heatwrap">';
    list.forEach((h, i) => {
      if (i === 0 || breaks.has(i)) {
        html += `<div class="heatgroup">${esc(h.group || 'ungrouped')}</div>`;
      }
      html += `<div class="heatrow" data-host="${esc(h.name)}">` +
              `<span class="hl">${esc(h.name)}</span><canvas></canvas>` +
              `<span class="hm"></span></div>`;
    });
    html += '<div class="heataxis"><span data-heat-from></span><span>now</span></div></div>';
    grid.innerHTML = html;

    for (const cv of grid.querySelectorAll('.heatrow canvas')) {
      // A cell that catches the eye is a question about one host, and History
      // is where it gets answered.
      cv.addEventListener('click', () => {
        prefs.slice = { mode: 'host', host: cv.closest('.heatrow').dataset.host };
        savePrefs();
        setView('history');
      });
      // ADR-011 reduces a bucket to its worst sample, so the reduction has to
      // be inspectable rather than a claim: hovering states the whole bucket.
      cv.addEventListener('mousemove', e => {
        const cells = cv._cells;
        if (!cells || !cells.length) return;
        const r = cv.getBoundingClientRect();
        const i = Math.max(0, Math.min(cells.length - 1,
          Math.floor((e.clientX - r.left) / r.width * cells.length)));
        const c = cells[i];
        const m = heatMetric();
        cv.title = c.v === null ? 'no sample in this slice'
          : `${m.fmt(c.min)}–${m.fmt(c.v)}, mean ${m.fmt(c.mean)} ` +
            `· ${c.n} sample${c.n === 1 ? '' : 's'}`;
      });
    }

    startHistoryTimer();
    refreshHeat();
  }

  async function refreshHeat() {
    if (prefs.view !== 'heat') return;
    const rows = [...grid.querySelectorAll('.heatrow')];
    if (!rows.length) return;

    const m = heatMetric();
    const id = prefs.heatMetric;
    const secs = sliderToSecs(+$('#histWindow').value);
    $('[data-hist-span]').textContent = 'last ' + fmtSpan(secs);
    const nowS = Math.floor(Date.now() / 1000);
    const win = { from: nowS - secs, to: nowS };
    // Never ask for more columns than the window can actually hold samples.
    // Sampling is at most 1 Hz, so a 60 s window has at most 60 distinct
    // readings; drawing it as 1200 pixel-wide cells invents 1140 empty ones
    // and then reports them as "95% gap" - a missing-data warning manufactured
    // entirely by the chart's own resolution. Wide windows are bounded by the
    // pixels instead, at three per cell.
    const px = rows[0].querySelector('canvas').clientWidth || 320;
    const cols = Math.max(20, Math.min(Math.round(px / 3), secs));

    let byHost = {};
    if (LIVE) {
      // One call for the fleet. Nineteen would be nineteen round trips per
      // redraw, and the slider redraws on every drag.
      try {
        byHost = await TAURI.core.invoke('query_history_fleet', {
          metric: id, fromSecsAgo: secs, toSecsAgo: 0, maxPoints: cols,
        });
      } catch (err) { showError(String(err)); return; }
    } else {
      for (const r of rows) byHost[r.dataset.host] = simHistory(r.dataset.host, id, secs, cols);
    }

    // One peak for the whole strip, so a colour means the same thing on every
    // row. Normalising each row against its own maximum would draw an idle
    // host exactly like the busiest one, destroying the comparison this view
    // exists to make - the same trap the log-scaled fleet bars avoid.
    const byRow = new Map();
    let peak = 0;
    for (const r of rows) {
      const cells = heatRow(byHost[r.dataset.host] || [], win, cols);
      byRow.set(r, cells);
      for (const c of cells) if (c.v !== null && c.v > peak) peak = c.v;
    }
    for (const r of rows) drawHeatRow(r, byRow.get(r), m, peak);

    $('[data-heat-from]').textContent = fmtSpan(secs) + ' ago';
    $('[data-heat-note]').textContent =
      `${m.label} · each cell is the peak in its slice, not its average`;
  }

  function drawHeatRow(row, cells, m, peak) {
    const cv = row.querySelector('canvas');
    cv._cells = cells;
    const w = cv.clientWidth, h = cv.clientHeight;
    if (!w || !h) return;
    const dpr = devicePixelRatio || 1;
    cv.width = Math.round(w * dpr);
    cv.height = Math.round(h * dpr);
    const ctx = cv.getContext('2d');
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);

    const gapFill = css('--heat-gap');
    const cw = w / cells.length;
    for (let i = 0; i < cells.length; i++) {
      const c = cells[i];
      if (c.v === null) {
        ctx.fillStyle = gapFill;
      } else {
        const t = m.scale === 'log' ? normalise(m, c.v, peak) : c.v / (m.max || 100);
        const r = ramp(t);
        ctx.fillStyle = mixHex(css(r.a), css(r.b), r.k);
      }
      // +1 so neighbours do not leave hairline seams at fractional widths.
      ctx.fillRect(i * cw, 0, cw + 1, h);
    }

    const cov = coverage(cells);
    const seen = cells.filter(c => c.v !== null);
    const rowPeak = seen.length ? Math.max(...seen.map(c => c.v)) : null;

    // Only a real deficit is called a gap. A column is one second and samples
    // arrive about once a second, so ordinary sub-second jitter - two landing
    // in one second, none in the next - leaves 2-10% of columns empty on a
    // perfectly healthy host. Flagging that made every row shout, which is the
    // same as no row shouting. Below 90% is a host actually delivering less
    // than it was asked for, which is worth seeing: measured against the real
    // fleet, towhee at 31 samples of 60 stands out while its neighbours at
    // 58-60 stay quiet.
    const GAP_FLOOR = 0.9;
    // "Quiet" and "not reporting" must still never look alike - the same rule
    // that keeps a fault from blanking a card.
    row.classList.toggle('gapped', cov < GAP_FLOOR);
    row.querySelector('.hm').textContent = rowPeak === null
      ? 'no data'
      : m.fmt(rowPeak) + (cov < GAP_FLOOR ? ` \u00b7 ${Math.round((1 - cov) * 100)}% gap` : '');
    const host = hosts.find(x => x.name === row.dataset.host);
    row.classList.toggle('faulted', !!(host && host.fault));
  }

  function startHistoryTimer() {
    clearInterval(histTimer);
    histTimer = setInterval(tickHistory, HIST_REFRESH_MS);
  }

  function stopHistoryTimer() {
    clearInterval(histTimer);
    histTimer = null;
  }

  /// One refresh at a time.
  ///
  /// A fleet-wide core view issues a query per host; if a pass takes longer
  /// than the interval, stacking them would queue work faster than it drains.
  async function tickHistory() {
    if (histBusy || document.hidden) return;
    if (prefs.view !== 'history' && prefs.view !== 'heat') return;
    histBusy = true;
    try {
      await (prefs.view === 'heat' ? refreshHeat() : refreshHistory());
    } catch (e) {
      console.error('history refresh failed', e);
    } finally {
      histBusy = false;
    }
  }

  async function refreshHistory() {
    if (prefs.view !== 'history') return;
    const secs = sliderToSecs(+$('#histWindow').value);
    $('[data-hist-span]').textContent = 'last ' + fmtSpan(secs);
    const nowS = Math.floor(Date.now() / 1000);
    const win = { from: nowS - secs, to: nowS };

    // Group charts are aggregated on read from their members' series, so a
    // group cannot drift from what it claims to summarise and re-labelling a
    // host re-labels its past with it.
    const gcharts = [...grid.querySelectorAll('.chart[data-group]')];
    await Promise.all(gcharts.map(async el => {
      const m = METRICS[el.dataset.metric];
      if (!m) return;
      const cv = el.querySelector('canvas');
      const budget = Math.max(60, Math.round(cv.clientWidth || 320));
      const list = hosts.filter(h => h.group === el.dataset.group);

      const series = await Promise.all(list.map(async h => {
        let points = [];
        if (LIVE) {
          try {
            points = await TAURI.core.invoke('query_history', {
              host: h.name, metric: el.dataset.metric,
              fromSecsAgo: secs, toSecsAgo: 0, maxPoints: budget,
            });
          } catch { points = []; }
        } else {
          points = simHistory(h.name, el.dataset.metric, secs, budget);
        }
        // Weight for 'ratio' metrics is the denominator the live view uses -
        // cores for CPU, gigabytes for memory - taken at its present value,
        // since history stores the percentage and not the parts it came from.
        const w = m.parts ? m.parts(h)[1] : 1;
        return { host: h.name, weight: w, points };
      }));

      const pts = TuxAgg.aggregateSeries(m, series, win, budget);
      drawHistory(cv, pts, m, win);

      const last = pts.length ? pts[pts.length - 1] : null;
      el.querySelector('[data-latest]').textContent = last ? m.fmt(last.mean) : '\u2014';
      // Say when the summary was incomplete, and for how much of the window.
      // A shaded span the reader has to interpret is not the same as being
      // told.
      const short = pts.filter(p => p.n < p.of).length;
      el.querySelector('[data-peak]').textContent = !pts.length ? ''
        : short
          ? `peak ${m.fmt(pts.reduce((a, p) => Math.max(a, p.max), 0))} · ` +
            `${Math.round(short / pts.length * 100)}% of window incomplete`
          : 'peak ' + m.fmt(pts.reduce((a, p) => Math.max(a, p.max), 0));
      el.classList.toggle('short', short > 0);
    }));

    const charts = [...grid.querySelectorAll('.chart:not([data-group])')];
    await Promise.all(charts.map(async el => {
      const m = metricFor(el.dataset.metric);
      if (!m) return;
      const cv = el.querySelector('canvas');
      const budget = Math.max(60, Math.round(cv.clientWidth || 320));

      let pts = [];
      if (LIVE) {
        try {
          pts = await TAURI.core.invoke('query_history', {
            host: el.dataset.host, metric: el.dataset.metric,
            fromSecsAgo: secs, toSecsAgo: 0, maxPoints: budget,
          });
        } catch { pts = []; }
      } else {
        pts = simHistory(el.dataset.host, el.dataset.metric, secs, budget);
      }

      drawHistory(cv, pts, m, win);
      const last = pts.length ? pts[pts.length - 1] : null;
      el.querySelector('[data-latest]').textContent = last ? m.fmt(last.mean) : '\u2014';
      el.querySelector('[data-peak]').textContent = pts.length
        ? 'peak ' + m.fmt(pts.reduce((a, p) => Math.max(a, p.max), 0)) : '';
    }));

    await refreshCoreCharts(secs, win);
  }

  /// Draw a min/max band with the mean over it.
  ///
  /// The band is the honest part: a coarse bucket that showed only its mean
  /// would hide exactly the spikes worth seeing.
  /// A gradient keyed to the value axis, so the high parts of a chart are red
  /// wherever in time they happen.
  ///
  /// Colouring a whole chart by one number would make a single 100% spike
  /// turn an otherwise idle hour red. Banding by height keeps the load
  /// meaning of the colour identical to the tiles and bars: blue below 75,
  /// amber to 89, red above.
  function bandedGradient(c, m, yOf, h) {
    const g = c.createLinearGradient(0, 0, 0, h);
    if (m.scale !== 'absolute' || (m.max || 100) !== 100) return null;

    const stop = v => Math.min(1, Math.max(0, yOf(v) / h));
    // Built top-down, which is high value to low.
    g.addColorStop(0, css('--crit'));
    g.addColorStop(stop(90), css('--crit'));
    g.addColorStop(stop(89.9), css('--warn'));
    g.addColorStop(stop(75), css('--warn'));
    g.addColorStop(stop(74.9), css('--accent'));
    g.addColorStop(1, css('--accent'));
    return g;
  }

  function drawHistory(cv, pts, m, win) {
    const dpr = devicePixelRatio || 1;
    const w = cv.clientWidth, h = cv.clientHeight;
    if (!w || !h) return;
    if (cv.width !== w * dpr || cv.height !== h * dpr) { cv.width = w * dpr; cv.height = h * dpr; }
    const c = cv.getContext('2d');
    c.setTransform(dpr, 0, 0, dpr, 0, 0);
    c.clearRect(0, 0, w, h);

    if (!pts.length) {
      c.fillStyle = css('--text-3');
      c.font = '11px ' + css('--font-mono');
      c.fillText('no history yet', 8, h / 2);
      return;
    }

    const peak = m.scale === 'absolute'
      ? (m.max || 100)
      : Math.max(1, pts.reduce((a, p) => Math.max(a, p.max), 0)) * 1.12;

    // The axis spans the requested window, not merely the data that exists.
    // Mapping the points across the full width stretched four minutes of
    // history over a chart labelled "last 7 days"; now partial history fills
    // the proportion it actually covers, at the right-hand edge.
    const t0 = win ? win.from : pts[0].t;
    const t1 = win ? win.to : pts[pts.length - 1].t;
    const span = Math.max(1, t1 - t0);
    const x = p => (p.t - t0) / span * w;
    const y = v => h - Math.min(1, Math.max(0, v / peak)) * (h - 4) - 2;

    c.strokeStyle = css('--stroke');
    c.lineWidth = 1;
    for (const f of [0.25, 0.5, 0.75]) {
      c.beginPath(); c.moveTo(0, h * f); c.lineTo(w, h * f); c.stroke();
    }

    // Where a group had fewer hosts reporting than it has, before anything is
    // drawn over it. Without this the line looks like a complete answer for a
    // span in which it was not one - and unlike a gap, a partial aggregate
    // draws a perfectly plausible value.
    const spans = [];
    pts.forEach((p, i) => {
      if (!(p.of > 0 && p.n < p.of)) return;
      const a = x(p);
      const b = i + 1 < pts.length ? x(pts[i + 1]) : w;
      // Merge with the previous span when they touch. Drawing one rect per
      // bucket instead leaves a picket fence: at a point per pixel the rects
      // land on fractional coordinates and antialias into stripes.
      const last = spans[spans.length - 1];
      if (last && a - last[1] < 1.5) last[1] = b;
      else spans.push([a, b]);
    });
    if (spans.length) {
      c.save();
      c.fillStyle = css('--warn');
      c.globalAlpha = 0.1;
      spans.forEach(([a, b]) => c.fillRect(a, 0, Math.max(1, b - a), h));
      c.globalAlpha = 0.55;
      spans.forEach(([a, b]) => c.fillRect(a, h - 2, Math.max(1, b - a), 2));
      c.restore();
    }

    const colour = m.scale === 'absolute' ? css('--accent') : css('--viz-net');
    const banded = bandedGradient(c, m, y, h);

    // Break the series wherever it skips.
    //
    // The backend stores a gap as NaN and drops it on query, so a host that
    // stopped reporting arrives as a *jump in t* rather than as a null. Drawn
    // naively that becomes a straight line across the outage - precisely the
    // "straight line implying it was fine throughout" that the storage layer's
    // own comment says it prevents. Storage did its job; drawing was undoing
    // it. Runs are split on a delta well past the normal spacing, so ordinary
    // jitter does not fragment a healthy chart.
    const deltas = pts.slice(1).map((p, i) => p.t - pts[i].t).sort((a, b) => a - b);
    const typical = deltas.length ? deltas[Math.floor(deltas.length / 2)] : 0;
    const maxStep = typical > 0 ? typical * 2.5 : Infinity;

    const runs = [[pts[0]]];
    for (let i = 1; i < pts.length; i++) {
      if (pts[i].t - pts[i - 1].t > maxStep) runs.push([]);
      runs[runs.length - 1].push(pts[i]);
    }

    // Say where the data is missing, rather than leaving blank space that
    // reads as "flat" or as the edge of the chart.
    if (runs.length > 1) {
      c.save();
      c.fillStyle = css('--text-3');
      c.globalAlpha = 0.07;
      for (let i = 1; i < runs.length; i++) {
        const a = x(runs[i - 1][runs[i - 1].length - 1]), b = x(runs[i][0]);
        c.fillRect(a, 0, Math.max(1, b - a), h);
      }
      c.restore();
    }

    // Area under the mean, always.
    //
    // The min/max band alone leaves short windows looking like a bare line:
    // those are served by the raw tier, where min == mean == max, so the band
    // has no height at all. The fill is what gives every zoom level the same
    // weight - and it is the part that reads as a chart rather than a wire.
    const meanPath = () => {
      for (const run of runs) {
        c.moveTo(x(run[0]), h);
        run.forEach(p => c.lineTo(x(p), y(p.mean)));
        c.lineTo(x(run[run.length - 1]), h);
      }
    };

    c.beginPath();
    meanPath();
    c.closePath();
    if (banded) {
      // Band the hue by height, then fade it downward so the fill still reads
      // as a fill rather than a solid block.
      c.save();
      c.clip();
      c.globalAlpha = 0.42;
      c.fillStyle = banded;
      c.fillRect(0, 0, w, h);
      c.globalAlpha = 1;
      const fade = c.createLinearGradient(0, 0, 0, h);
      fade.addColorStop(0, 'transparent');
      fade.addColorStop(1, css('--surface-sunk'));
      c.fillStyle = fade;
      c.fillRect(0, 0, w, h);
      c.restore();
    } else {
      const fill = c.createLinearGradient(0, 0, 0, h);
      fill.addColorStop(0, colour + '66');
      fill.addColorStop(0.55, colour + '2A');
      fill.addColorStop(1, colour + '05');
      c.fillStyle = fill;
      c.fill();
    }

    // The min/max band on top, where the tier has spread to show.
    const spread = pts.some(p => p.max - p.min > 1e-6);
    if (spread) {
      c.beginPath();
      for (const run of runs) {
        run.forEach((p, i) => (i ? c.lineTo(x(p), y(p.max)) : c.moveTo(x(p), y(p.max))));
        for (let i = run.length - 1; i >= 0; i--) c.lineTo(x(run[i]), y(run[i].min));
      }
      c.closePath();
      const band = c.createLinearGradient(0, 0, 0, h);
      band.addColorStop(0, colour + '4D');
      band.addColorStop(1, colour + '14');
      c.fillStyle = band;
      c.fill();
    }

    // A specular sheen along the top, matching the tiles and bars.
    c.save();
    c.beginPath();
    meanPath();
    c.closePath();
    c.clip();
    const gloss = c.createLinearGradient(0, 0, 0, h * 0.55);
    gloss.addColorStop(0, css('--gloss'));
    gloss.addColorStop(1, 'transparent');
    c.fillStyle = gloss;
    c.fillRect(0, 0, w, h);
    c.restore();

    // Mean line last, so it sits above its own fill.
    c.beginPath();
    for (const run of runs) {
      run.forEach((p, i) => (i ? c.lineTo(x(p), y(p.mean)) : c.moveTo(x(p), y(p.mean))));
    }
    c.strokeStyle = banded || colour; c.lineWidth = 1.5; c.lineJoin = 'round'; c.stroke();

    const lastP = pts[pts.length - 1];
    const lx = x(lastP), ly = y(lastP.mean);
    const dot = banded ? bandColour(lastP.mean) : colour;
    c.beginPath(); c.arc(lx, ly, 2.6, 0, 7); c.fillStyle = dot; c.fill();
    c.beginPath(); c.arc(lx, ly, 5.2, 0, 7);
    c.strokeStyle = dot + '55'; c.lineWidth = 1.4; c.stroke();
  }

  const bandColour = v =>
    css(v >= 90 ? '--crit' : v >= 75 ? '--warn' : '--accent');

  /// Browser-mode history, so the page still demonstrates itself as a mockup.
  function simHistory(host, metric, secs, budget) {
    const h = hosts.find(x => x.name === host);
    const base = h ? (last(h.hist.cpu) || 10) : 10;
    const n = Math.min(budget, 200);
    const now = Math.floor(Date.now() / 1000);
    return Array.from({ length: n }, (_, i) => {
      const t = now - secs + Math.round(i * secs / n);
      const mean = Math.max(0, base + Math.sin(i / 7) * 8 + Math.random() * 4);
      return { t, min: Math.max(0, mean - 4), mean, max: mean + 6 + (i % 23 === 0 ? 30 : 0) };
    });
  }

  $('#histSubject').addEventListener('change', e => {
    const s = currentSlice();
    prefs.slice = s.mode === 'host'
      ? { mode: 'host', host: e.target.value }
      : { mode: 'metric', metric: e.target.value };
    savePrefs();
    build();
  });

  $('#histWindow').value = String(prefs.histWin ?? 0);
  $('#histWindow').addEventListener('input',
    () => (prefs.view === 'heat' ? refreshHeat() : refreshHistory()));
  // Saved on `change`, not `input`: dragging fires input per pixel, and
  // writing localStorage on every one of those to record a position the user
  // is still choosing is pure waste.
  $('#histWindow').addEventListener('change', e => {
    prefs.histWin = +e.target.value;
    savePrefs();
  });
  $('#histSwap').addEventListener('click', () => {
    const s = currentSlice();
    prefs.slice = s.mode === 'host'
      ? { mode: 'metric', metric: scalarMetricId(prefs.metric) }
      : { mode: 'host', host: ordered()[0]?.name };
    savePrefs();
    build();
  });

  // ---- processes ---------------------------------------------------------
  //
  // One list, every host, sorted by CPU then memory. The question this answers
  // is "what is the busiest process anywhere", which needs no host chosen
  // first - that is the part Task Manager cannot do.

  const PROC_REFRESH_MS = 2000;
  let procTimer = null, procBusy = false;

  function startProcs() {
    if (LIVE) TAURI.core.invoke('set_processes_enabled', { enabled: true })
      .catch(err => showError(String(err)));
    clearInterval(procTimer);
    procTimer = setInterval(tickProcs, PROC_REFRESH_MS);
  }

  function stopProcs() {
    clearInterval(procTimer);
    procTimer = null;
    // Sampling costs a second of remote wall clock per host per cycle, so a
    // view nobody is looking at must cost nothing at all.
    if (LIVE) TAURI.core.invoke('set_processes_enabled', { enabled: false }).catch(() => {});
  }

  async function tickProcs() {
    if (procBusy || prefs.view !== 'procs' || document.hidden) return;
    procBusy = true;
    try { await refreshProcs(); } catch (e) { console.error('processes', e); }
    finally { procBusy = false; }
  }

  /// Columns, and how each one sorts.
  ///
  /// Numeric columns default to descending because the interesting end of a
  /// process table is the top; text columns default to ascending because the
  /// interesting thing there is finding a name.
  const PROC_COLS = [
    { key: 'host', label: 'Host', cls: 'host', num: false },
    { key: 'cpu_pct', label: 'CPU', cls: 'num', num: true },
    { key: 'rss_kb', label: 'Memory', cls: 'num', num: true },
    { key: 'pid', label: 'PID', cls: 'num', num: true },
    { key: 'user', label: 'User', cls: '', num: false },
    { key: 'owner', label: 'Owner', cls: 'owner', num: false },
    { key: 'comm', label: 'Command', cls: 'cmd', num: false },
  ];

  function buildProcs() {
    startProcs();
    grid.innerHTML = '';
    $('#procbar').hidden = false;
    $('#procKernel').checked = !!prefs.procKernel;
    $('#procByOwner').checked = !!prefs.procByOwner;
    $('#procFilter').value = procFilter;

    const head = PROC_COLS.map(c => {
      const active = prefs.procSort === c.key;
      const arrow = active ? (prefs.procDesc ? '\u25bc' : '\u25b2') : '';
      return `<th class="${c.cls}" data-col="${c.key}" tabindex="0"
        ${active ? `aria-sort="${prefs.procDesc ? 'descending' : 'ascending'}"` : ''}
        >${c.label}<span class="arrow">${arrow}</span></th>`;
    }).join('');

    grid.innerHTML = `
      <table class="proctable">
        <thead><tr>${head}</tr></thead>
        <tbody data-proc-rows></tbody>
      </table>`;

    grid.querySelectorAll('th[data-col]').forEach(th => {
      const go = () => sortProcsBy(th.dataset.col);
      th.addEventListener('click', go);
      th.addEventListener('keydown', e => {
        if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); go(); }
      });
    });

    refreshProcs();
  }

  function sortProcsBy(key) {
    const col = PROC_COLS.find(c => c.key === key);
    if (!col) return;
    if (prefs.procSort === key) {
      prefs.procDesc = !prefs.procDesc;
    } else {
      prefs.procSort = key;
      prefs.procDesc = col.num;   // numbers open at the busy end
    }
    savePrefs();
    build();   // headers carry the indicator, so they are rebuilt too
  }

  /// Filter is transient: it answers a question being asked now, and having a
  /// stale one restored on a later launch would hide rows for no visible
  /// reason.
  let procFilter = '';

  /// Which rows are showing their command line, keyed by host:pid.
  ///
  /// Also transient, and keyed by pid rather than by row index because the
  /// table re-sorts every few seconds - an index would expand whichever
  /// process happened to land in that position next.
  const procOpen = new Set();

  /// Current sort preferences applied to a list. The comparison itself lives
  /// in the filter module; this only supplies which column and which way.
  function sortProcs(list) {
    const key = prefs.procSort || 'cpu_pct';
    const col = PROC_COLS.find(c => c.key === key) || PROC_COLS[1];
    return TuxFilter.sortProcs(list, key, prefs.procDesc !== false, col.num);
  }

  /// Cgroup accounting from the last sample, keyed `host\u2022owner`.
  ///
  /// A printable separator, deliberately: a NUL is a fine Map key and is
  /// silently dropped from an HTML attribute, so the rendered `data-proc`
  /// never matched the key the expand toggle had stored - the row simply did
  /// not open, with no error anywhere.
  let cgroups = new Map();

  /// Render processes grouped by what owns them.
  ///
  /// Ranked by the *cgroup's* CPU and memory rather than by the processes we
  /// happen to hold: a service can be the largest thing on a box while none of
  /// its individual processes reaches the top twenty. That is precisely the
  /// case the process list cannot show, and the reason this view exists.
  function ownerRows(list) {
    const byOwner = new Map();
    for (const p of list) {
      if (!p.owner) continue;
      const k = `${p.host}\u2022${p.owner}`;
      if (!byOwner.has(k)) byOwner.set(k, []);
      byOwner.get(k).push(p);
    }

    const rows = [];
    const seen = new Set();
    for (const [k, u] of cgroups) {
      rows.push({ key: k, host: u.host, owner: u.name, usage: u, procs: byOwner.get(k) || [] });
      seen.add(k);
    }
    // Owners we have processes for but no cgroup reading - a host on cgroup
    // v1, or a container outside system.slice. Shown without the numbers
    // rather than dropped, since the processes are real.
    for (const [k, ps] of byOwner) {
      if (!seen.has(k)) rows.push({ key: k, host: ps[0].host, owner: ps[0].owner, usage: null, procs: ps });
    }

    return rows.sort((a, b) =>
      ((b.usage?.cpu_pct || 0) - (a.usage?.cpu_pct || 0)) ||
      ((b.usage?.memory_bytes || 0) - (a.usage?.memory_bytes || 0)) ||
      a.owner.localeCompare(b.owner));
  }

  /// Draw the grouped view: one row per owner, expanding to its processes.
  function renderOwnerRows(list, body, total, q) {
    const rows = ownerRows(list);
    if (!rows.length) {
      body.innerHTML = `<tr><td colspan="7">No owners reported yet — cgroup
        accounting needs a cgroup v2 host, and the first sample carries no CPU
        because the counter needs two.</td></tr>`;
      $('[data-proc-note]').textContent = '';
      return;
    }

    body.innerHTML = rows.map(r => {
      const open = procOpen.has(r.key);
      const u = r.usage;
      const kids = open ? sortProcs(r.procs).map(p => `
        <tr class="owner-child">
          <td class="host"></td>
          <td class="num" data-band="${band(p.cpu_pct)}">${p.cpu_pct.toFixed(1)}%</td>
          <td class="num">${fmtKb(p.rss_kb)}</td>
          <td class="num">${p.pid}</td>
          <td>${esc(p.user)}</td>
          <td class="owner"></td>
          <td class="cmd">${esc(p.comm)}</td>
        </tr>`).join('') : '';

      return `
      <tr class="owner-row${open ? ' open' : ''}" data-proc="${esc(r.key)}"
          tabindex="0" role="button" aria-expanded="${open}">
        <td class="host">${esc(r.host)}</td>
        <td class="num" ${u ? `data-band="${band(u.cpu_pct)}"` : ''}>${u ? u.cpu_pct.toFixed(1) + '%' : '—'}</td>
        <td class="num" title="cgroup charge, which counts page cache — not the sum of the processes' memory">${u ? fmtBytes(u.memory_bytes) : '—'}</td>
        <td class="num">${u ? u.pids : r.procs.length}</td>
        <td></td>
        <td class="owner" colspan="2">${esc(r.owner)}${restartBadge(u)}</td>
      </tr>${kids}`;
    }).join('');

    const hosts_seen = new Set(rows.map(r => r.host)).size;
    // The memory column means something different here, and saying so is the
    // difference between a useful number and a misleading one.
    $('[data-proc-note]').textContent =
      `${rows.length} owner${rows.length === 1 ? '' : 's'} across ` +
      `${hosts_seen} host${hosts_seen === 1 ? '' : 's'} \u00b7 ` +
      `CPU is % of the whole machine \u00b7 memory is the cgroup charge, ` +
      `which includes page cache`;
  }

  /// A unit's restart count, shown only when it has restarted.
  ///
  /// `NRestarts` carries no recency - it counts since the unit was last
  /// started explicitly, which may be months ago - so a bare "7 restarts"
  /// beside a live CPU chart implies a today it does not mean. The badge is
  /// warning-toned only when restarts happened *while Tuxtop was watching*,
  /// which is the half that means flapping now.
  function restartBadge(u) {
    if (!u || !u.restarts) return '';
    const since = u.restarts_since_seen || 0;
    const label = since
      ? `${u.restarts} restarts · +${since} while watching`
      : `${u.restarts} restart${u.restarts === 1 ? '' : 's'}`;
    const title = since
      ? `Restarted ${since} time${since === 1 ? '' : 's'} since Tuxtop started watching — this unit is flapping now`
      : `systemd reports ${u.restarts} automatic restart${u.restarts === 1 ? '' : 's'} since this unit was last started by hand, which may have been long ago`;
    return ` <span class="restarts${since ? ' live' : ''}" title="${esc(title)}">${esc(label)}</span>`;
  }

  /// Bytes, for cgroup memory. Distinct from fmtKb, which takes kilobytes.
  function fmtBytes(b) {
    return b >= 1073741824 ? `${(b / 1073741824).toFixed(1)} GB`
         : b >= 1048576 ? `${Math.round(b / 1048576)} MB`
         : `${Math.round(b / 1024)} KB`;
  }

  /// Browser-mode cgroups, derived from the simulated processes so the two
  /// agree with each other.
  function simCgroups(list) {
    const m = new Map();
    for (const p of list) {
      if (!p.owner) continue;
      const k = `${p.host}\u2022${p.owner}`;
      const cur = m.get(k) || { host: p.host, name: p.owner, cpu_pct: 0, memory_bytes: 0, pids: 0 };
      cur.cpu_pct += p.cpu_pct;
      // Deliberately above the RSS sum: the real memory.current counts page
      // cache, and a simulator that matched exactly would hide that.
      cur.memory_bytes += p.rss_kb * 1024 * 1.35;
      cur.pids += 1;
      m.set(k, cur);
    }
    return m;
  }

  async function refreshProcs() {
    if (prefs.view !== 'procs') return;
    let list = [];
    if (LIVE) {
      try { list = await TAURI.core.invoke('process_list'); } catch { list = []; }
      try {
        const cg = await TAURI.core.invoke('cgroup_list');
        cgroups = new Map(cg.map(u => [`${u.host}\u2022${u.name}`, u]));
      } catch { cgroups = new Map(); }
    } else {
      list = simProcs();
      cgroups = simCgroups(list);
    }

    if (!prefs.procKernel) list = list.filter(p => !p.kernel);

    const total = list.length;
    const q = procFilter.trim().toLowerCase();
    if (q) list = list.filter(p => matchesProcess(p, q));
    list = sortProcs(list);

    const body = $('[data-proc-rows]');
    if (!body) return;

    if (!list.length && q) {
      body.innerHTML = `<tr><td colspan="7">Nothing matches
        \u201c${esc(procFilter)}\u201d among ${total} processes.</td></tr>`;
      $('[data-proc-note]').textContent = `0 of ${total} shown`;
      return;
    }

    if (!list.length) {
      body.innerHTML = `<tr><td colspan="7">Sampling — first results take a
        few seconds, since each host measures CPU over a one-second window.</td></tr>`;
      $('[data-proc-note]').textContent = '';
      return;
    }

    if (prefs.procByOwner) return renderOwnerRows(list, body, total, q);

    body.innerHTML = list.map(p => {
      const key = `${p.host}:${p.pid}`;
      const open = procOpen.has(key);
      // comm is capped at 15 characters by the kernel, so the short name and
      // the command line are genuinely different information - the row shows
      // the name, expanding shows what it actually is.
      const detail = open ? `
        <tr class="proc-detail">
          <td colspan="7"><code>${esc(p.cmd || 'no command line — kernel threads have none')}</code></td>
        </tr>` : '';
      return `
      <tr class="${p.kernel ? 'kernel' : ''}${p.cmd ? ' has-cmd' : ''}${open ? ' open' : ''}"
          ${p.cmd ? `data-proc="${esc(key)}" tabindex="0" role="button"
             aria-expanded="${open}" title="Show the full command line"` : ''}>
        <td class="host">${esc(p.host)}</td>
        <td class="num" data-band="${band(p.cpu_pct)}">${p.cpu_pct.toFixed(1)}%</td>
        <td class="num">${fmtKb(p.rss_kb)}</td>
        <td class="num">${p.pid}</td>
        <td>${esc(p.user)}</td>
        <td class="owner" data-kind="${esc(p.owner_kind || 'none')}"
            title="${esc(p.owner || 'no cgroup — a kernel thread, or the process ended')}">${esc(p.owner || '—')}</td>
        <td class="cmd">${esc(p.comm)}</td>
      </tr>${detail}`;
    }).join('');

    const hosts_seen = new Set(list.map(p => p.host)).size;
    // Saying which convention is in use matters: top would call the same
    // process 3200% on a 32-core box. And when a filter is active, say how
    // much is being hidden - a filtered count that looks like a total is a
    // quiet way to mislead.
    const shown = q ? `${list.length} of ${total} processes` : `${list.length} processes`;
    $('[data-proc-note]').textContent =
      `${shown} across ${hosts_seen} host${hosts_seen === 1 ? '' : 's'} ` +
      `\u00b7 CPU is % of the whole machine`;
  }

  /// Browser-mode processes, so the page still demonstrates itself.
  function simProcs() {
    const names = ['tailscaled', 'searchd', 'python', 'node', 'postgres',
                   'kworker/3:1', 'dockerd', 'nginx', 'redis-server', 'java'];
    return hosts.flatMap(h => names.slice(0, 6).map((n, i) => ({
      host: h.name, pid: 1000 + i * 37, cpu_pct: Math.max(0, 40 - i * 7 + Math.random() * 6),
      rss_kb: (900 - i * 120) * 1024, user: i % 3 ? 'root' : 'sam', comm: n,
      kernel: n.startsWith('kworker'),
    }))).sort((a, b) => b.cpu_pct - a.cpu_pct || b.rss_kb - a.rss_kb);
  }

  // Delegated, because the table is rebuilt on every refresh.
  grid.addEventListener('click', e => {
    const row = e.target.closest('tr[data-proc]');
    if (!row) return;
    toggleProc(row.dataset.proc);
  });
  grid.addEventListener('keydown', e => {
    if (e.key !== 'Enter' && e.key !== ' ') return;
    const row = e.target.closest('tr[data-proc]');
    if (!row) return;
    e.preventDefault();          // Space would otherwise scroll the table
    toggleProc(row.dataset.proc);
  });

  function toggleProc(key) {
    if (procOpen.has(key)) procOpen.delete(key);
    else procOpen.add(key);
    refreshProcs();
  }

  $('#hostFilter').addEventListener('input', e => {
    hostFilter = e.target.value;
    build();
    paint();
  });
  // Escape clears rather than merely blurring, so getting back to the whole
  // fleet is one key and never a hunt for the right end of the text.
  $('#hostFilter').addEventListener('keydown', e => {
    if (e.key !== 'Escape') return;
    e.stopPropagation();
    if (!e.target.value) return;
    e.target.value = '';
    hostFilter = '';
    build();
    paint();
  });

  $('#procFilter').addEventListener('input', e => {
    procFilter = e.target.value;
    refreshProcs();
  });

  $('#procByOwner').addEventListener('change', e => {
    prefs.procByOwner = e.target.checked;
    savePrefs();
    procOpen.clear();   // keys differ between the two shapes
    build();
  });

  $('#procKernel').addEventListener('change', e => {
    prefs.procKernel = e.target.checked; savePrefs(); refreshProcs();
  });

  // ---- settings ----------------------------------------------------------
  const INTERVALS = [1, 2, 5, 10, 30, 60];

  /// Bytes per second the fleet would cost at `iv`, from measured frame sizes.
  ///
  /// Arithmetic, not estimation: frame size tracks disk and interface count
  /// rather than load, so it is effectively constant per host and the rate at
  /// interval I really is size over I. A host's own override is honoured, so
  /// changing the global rate does not claim to change hosts it will not touch.
  function projectFleet(rows, globalIv, overrides) {
    return rows.reduce((sum, r) => {
      const mean = r.frames_total ? r.bytes_total / r.frames_total : 0;
      const iv = overrides.get(r.host) ?? globalIv;
      return sum + (iv > 0 ? mean / iv : 0);
    }, 0);
  }

  const perDay = bytesPerSec => bytesPerSec * 86400;

  function fmtDay(b) {
    return b >= 1024 ** 3 ? `${(b / 1024 ** 3).toFixed(1)} GB/day`
         : b >= 1024 ** 2 ? `${(b / 1024 ** 2).toFixed(0)} MB/day`
         : `${(b / 1024).toFixed(0)} KB/day`;
  }

  async function refreshMeter() {
    if (!LIVE) return;
    const { invoke } = TAURI.core;
    let rows = [];
    try { rows = await invoke('traffic_stats'); } catch { return; }

    const reporting = rows.filter(r => r.frames_total > 0);
    const overrides = new Map(
      hosts.filter(h => h.intervalOverride).map(h => [h.name, h.intervalOverride]));
    const chosen = +$('#s-interval').value;

    const now = projectFleet(reporting, chosen, overrides);
    $('[data-meter-now]').textContent = reporting.length
      ? `${bps(now)}  ${fmtDay(perDay(now))}`
      : 'no samples yet';

    // Every interval, so the cost of the choice is visible before making it.
    $('[data-meter-rows]').innerHTML = INTERVALS.map(iv => {
      const b = projectFleet(reporting, iv, overrides);
      return `<tr class="${iv === chosen ? 'current' : ''}">
        <td>${iv === 1 ? '1 second' : iv < 60 ? iv + ' seconds' : '1 minute'}</td>
        <td>${bps(b)}</td><td>${fmtDay(perDay(b))}</td></tr>`;
    }).join('');

    const overridden = overrides.size;
    $('[data-meter-note]').textContent =
      `Measured across ${reporting.length} reporting host${reporting.length === 1 ? '' : 's'}` +
      (overridden ? `, ${overridden} with an override the global rate will not change.` : '.');

    // What the memory budget actually buys.
    //
    // Measured, not estimated. This panel used to multiply a series count by a
    // hardcoded 79.9 KB and report the product as fact, while `history_usage`
    // sat in the backend returning the real figure and was never called. An
    // app that exists because an agent reported a plausible wrong number has
    // no business doing the same about itself - and the traffic meter directly
    // above says "measured" in earnest.
    const cap = +$('#s-cap').value;
    let usage = null;
    if (LIVE) {
      try { usage = await TAURI.core.invoke('history_usage'); } catch { usage = null; }
    }

    const hint = $('[data-cap-hint]');
    if (!usage || !usage.series) {
      hint.textContent = 'Held in memory only; a restart starts clean.';
    } else {
      const mb = b => b / 1048576;
      const held = mb(usage.bytes);
      // Where a full fleet lands, since every series is bounded by
      // construction. This one is a projection and is worded as one.
      const full = mb(usage.series * usage.full_series_bytes);
      const parts = [
        `Holding ${held < 1 ? held.toFixed(1) : held.toFixed(0)} MB across ` +
        `${usage.series} series; full, that becomes about ${full.toFixed(0)} MB.`,
      ];
      if (usage.finest_secs > 1) {
        // The cap is doing something. Saying so matters more than the number:
        // charts quietly getting coarser with no explanation is the kind of
        // silent degradation this app is supposed to make impossible.
        parts.push(`Over the limit, so detail was dropped - the finest history ` +
                   `now kept is ${usage.finest_secs}s, not 1s. Raise the limit to keep more.`);
      } else {
        parts.push(full <= cap
          ? 'Within the limit, so nothing is dropped.'
          : 'Projected to exceed the limit, at which point the finest detail is dropped first.');
      }
      hint.textContent = parts.join(' ');
    }
  }

  function perHostRows() {
    // Existing groups offered as suggestions, so "servers" and "Servers" do
    // not silently become two groups that look like one.
    const known = [...new Set(hosts.map(h => h.group).filter(Boolean))].sort();
    $('#phGroups').innerHTML = known.map(g => `<option value="${esc(g)}">`).join('');

    $('[data-perhost-rows]').innerHTML = hosts.map(h => `
      <tr><td>${esc(h.name)}</td><td>
        <select data-host-iv="${esc(h.name)}">
          <option value="">follow global</option>
          ${INTERVALS.map(iv =>
            `<option value="${iv}"${h.intervalOverride === iv ? ' selected' : ''}>${iv}s</option>`
          ).join('')}
        </select></td><td>
        <input class="ph-group" list="phGroups" data-host-group="${esc(h.name)}"
               value="${esc(h.group || '')}" placeholder="none" autocomplete="off"
               aria-label="Group for ${esc(h.name)}">
        </td><td>
        <select data-host-os="${esc(h.name)}" aria-label="Operating system for ${esc(h.name)}">
          <option value=""${h.os ? '' : ' selected'}>Linux</option>
          <option value="windows"${h.os === 'windows' ? ' selected' : ''}>Windows</option>
        </select>
        </td></tr>`).join('');
  }

  const setDlg = $('#setDlg');
  let meterTimer = null;

  $('#settingsBtn').addEventListener('click', async () => {
    if (LIVE) {
      try {
        const s = await TAURI.core.invoke('get_settings');
        $('#s-interval').value = String(s.interval_secs);
        $('#s-cap').value = String(s.history_cap_mb);
        $('#s-ontop').checked = !!s.always_on_top;
      } catch (e) { showError(String(e)); }
    }
    perHostRows();
    await refreshMeter();
    setDlg.showModal();
    // The meter is live while the dialog is open; measurements keep arriving.
    clearInterval(meterTimer);
    meterTimer = setInterval(refreshMeter, 2000);
  });

  setDlg.addEventListener('close', () => clearInterval(meterTimer));
  $('#s-ontop').addEventListener('change', async e => {
    if (!LIVE) return;
    try {
      const s = await TAURI.core.invoke('get_settings');
      await TAURI.core.invoke('set_settings', {
        settings: { ...s, always_on_top: e.target.checked },
      });
    } catch (err) { showError(String(err)); }
  });

  $('#s-interval').addEventListener('change', refreshMeter);
  $('#s-cap').addEventListener('change', refreshMeter);

  $('#setForm').addEventListener('submit', async e => {
    if (e.submitter && e.submitter.value !== 'save') return;
    if (!LIVE) return;
    try {
      await TAURI.core.invoke('set_settings', { settings: {
        interval_secs: +$('#s-interval').value,
        history_cap_mb: +$('#s-cap').value,
        always_on_top: $('#s-ontop').checked,
      }});
    } catch (err) { showError(String(err)); }
  });

  // Per-host overrides apply immediately - the host restarts at the new rate
  // and the meter updates, so the effect is visible while the dialog is open.
  $('[data-perhost-rows]').addEventListener('change', async e => {
    const sel = e.target.closest('[data-host-iv]');
    if (!sel || !LIVE) return;
    const name = sel.dataset.hostIv;
    const v = sel.value === '' ? null : +sel.value;
    const h = hosts.find(x => x.name === name);
    if (h) h.intervalOverride = v;
    try {
      await TAURI.core.invoke('set_host_interval', { name, intervalSecs: v });
    } catch (err) { showError(String(err)); }
    refreshMeter();
  });

  // Committed on blur or Enter rather than per keystroke: every change
  // rewrites hosts.toml and re-renders the fleet, and doing that once per
  // typed letter would rearrange the view under the cursor.
  setDlg.addEventListener('change', async e => {
    const sel = e.target.closest('[data-host-os]');
    if (sel && LIVE) {
      const name = sel.dataset.hostOs;
      try {
        await TAURI.core.invoke('set_host_os', { name, os: sel.value });
        const h = hosts.find(x => x.name === name);
        if (h) h.os = sel.value;
      } catch (err) { showError(String(err)); }
      return;
    }
    const inp = e.target.closest('[data-host-group]');
    if (!inp || !LIVE) return;
    const name = inp.dataset.hostGroup;
    const group = inp.value.trim() || null;
    try {
      await TAURI.core.invoke('set_host_group', { name, group });
      const h = hosts.find(x => x.name === name);
      if (h) h.group = group;
      perHostRows();          // refresh the suggestion list with any new group
      build(); paint();
    } catch (err) { showError(String(err)); }
  });

  const dlg = $('#addDlg');
  $('#addBtn').addEventListener('click', () => {
    // <form method="dialog"> does not reset on close, so reopening kept the
    // previous host's details - submit again and you get a duplicate-name
    // rejection for a host you thought you were adding fresh.
    $('#addForm').reset();
    // Offer the groups that already exist. Typing a new one is still allowed -
    // a datalist suggests, it does not constrain - but suggesting the existing
    // spelling is what stops "workstations" and "Workstations" becoming two
    // groups that look like one.
    const dl = $('#groupList');
    dl.innerHTML = '';
    [...new Set(hosts.map(h => h.group).filter(Boolean))].forEach(g => {
      const o = document.createElement('option');
      o.value = g;
      dl.appendChild(o);
    });
    dlg.showModal();
    $('#f-name').focus();
    $('#f-name').select();
  });
  addEventListener('resize', () => paint());

  // Reveal highlight. Windows' own Fluent surfaces light up under the
  // cursor; matching that makes the app read as more native, not less.
  grid.addEventListener('pointermove', e => {
    // Every panel type, not just the live views. The reveal was scoped to
    // .card and .hostblock, so History - which uses .chart, .cores-hist and
    // .core-chart - had no lighting at all.
    const panel = e.target.closest('.card, .hostblock, .chart, .cores-hist, .core-chart');
    if (!panel) return;
    const r = panel.getBoundingClientRect();
    panel.style.setProperty('--mx', (e.clientX - r.left) + 'px');
    panel.style.setProperty('--my', (e.clientY - r.top) + 'px');
  }, { passive: true });

  // ---------------------------------------------------------------- LIVE
  async function startLive() {
    const { invoke } = TAURI.core;
    const { listen } = TAURI.event;

    // The cadence toggle was a mockup device for showing why the fast plane
    // exists. Against a real backend it has nothing to switch, so it goes.
    $('#rateFast').closest('.seg').remove();
    $('#cadNote').remove();
    // Core count is discovered from the first sample, so asking for it would
    // be asking the user to tell us something we are about to find out.
    $('#f-cores').closest('.field').remove();
    // Windows draws the real titlebar (decorations: true), so the mockup's
    // painted one would be a second, fake title bar stacked under it.
    document.querySelector('.titlebar')?.remove();
    const note = document.querySelector('[data-mode-note]');
    if (note) note.textContent = 'live \u00b7 1 Hz over ssh';

    const ensure = (name, nCores) => {
      let h = hosts.find(x => x.name === name);
      if (!h) { h = mk(name, '', nCores || 0, 0, null, 0); hosts.push(h); build(); }
      else if (nCores && h.cores !== nCores) { h.cores = nCores; build(); }
      return h;
    };

    await listen('tuxtop://sample', ({ payload: s }) => {
      const h = ensure(s.host, s.cores.length);
      h.fault = null;
      h.seen = true;   // it has actually reported, so it can count as up
      h.core = s.cores;
      h.ramGB = s.mem_total_kb / 1048576;
      h.ram = s.mem_used_kb / 1048576;
      h.net = s.net_rx_bps + s.net_tx_bps;      // bytes/sec
      h.dio = s.disk_read_bps + s.disk_write_bps; // bytes/sec
      h.load = s.load;
      h.temp = (typeof s.cpu_temp_c === 'number') ? s.cpu_temp_c : null;
      // Every sensor, named. Kept separate from h.temp because that one is
      // the reading the CPU ranking vouches for; the hottest thing in the box
      // is frequently an NVMe and must not be confused with it.
      h.temps = Array.isArray(s.temps) ? s.temps.map((t, i, all) => {
        const idx = all.slice(0, i).filter(x => x.driver === t.driver).length;
        return { ...t, idx, name: sensorName(t, idx) };
      }) : [];
      h.uptime = s.uptime_secs ?? h.uptime ?? null;
      h.swapUsed = s.swap_used_kb || 0;
      h.swapTotal = s.swap_total_kb || 0;
      h.breakdown = s.cpu_breakdown || null;
      // Sent once per connection and on a slow cadence respectively; a frame
      // without them means "unchanged", not "gone".
      if (s.facts) h.facts = s.facts;
      if (s.filesystems && s.filesystems.length) h.fs = s.filesystems;
      if (h.temp !== null) push(h.hist.temp, h.temp);
      if (s.gpu) {
        h.gpu = s.gpu.name;
        h.gpuU = s.gpu.util_pct;
        h.gpuUsed = s.gpu.mem_used_mb;
        h.gpuTotal = s.gpu.mem_total_mb;
        h.gpuW = s.gpu.power_w;
      }
      push(h.hist.cpu, s.cpu);
      push(h.hist.ram, h.ramGB ? h.ram / h.ramGB * 100 : 0);
      push(h.hist.dio, h.dio);
      push(h.hist.net, h.net);
      push(h.hist.gpu, h.gpuU || 0);
      paint(); tally();
    });

    await listen('tuxtop://fault', ({ payload: f }) => {
      if (!f || !f.host) return;
      const h = ensure(f.host, 0);
      h.fault = f;
      paint(); tally();
    });

    await listen('tuxtop://hosts-changed', ({ payload: list }) => {
      // Remember each host's interval override so the meter can honour it,
      // and its group so the fleet view can arrange by it.
      const cfgs = new Map(list.map(c => [c.name, c]));
      const apply = h => {
        const c = cfgs.get(h.name);
        h.intervalOverride = c ? c.interval_secs ?? null : null;
        h.group = c ? c.group ?? null : null;
        h.os = c ? c.os ?? '' : '';
      };
      hosts.forEach(apply);
      // Reconcile both ways. Filtering alone only ever removed, so a newly
      // added host got no card until its first sample arrived - and an
      // unreachable host never sends one, so it stayed invisible for the
      // full ssh connect timeout or forever. Order follows the backend list.
      const byName = new Map(hosts.map(h => [h.name, h]));
      hosts = list.map(c => byName.get(c.name) || mk(c.name, '', 0, 0, null, 0));
      hosts.forEach(apply);
      build(); paint(); tally();
    });

    // Seed cards from hosts.toml so they exist before the first sample
    // lands -- otherwise the window is empty for a second on every launch.
    try {
      for (const cfg of await invoke('list_hosts')) {
        const h = ensure(cfg.name, 0);
        h.intervalOverride = cfg.interval_secs ?? null;
        h.group = cfg.group ?? null;
        h.os = cfg.os ?? '';
      }
    } catch (e) {
      showError(`Could not read hosts.toml: ${e}`);
      return;
    }

    // A read-only server will refuse configuration changes, so the controls
    // that make them are hidden rather than left to fail. Absent on the
    // desktop app, where everything is allowed.
    try {
      const caps = await invoke('capabilities');
      if (caps && caps.writable === false) {
        document.body.dataset.readonly = 'yes';
      }
    } catch { /* no capabilities command: the desktop app, which can do it all */ }

    build(); paint(); tally();
    if (!hosts.length) showEmpty('No hosts yet. Add one to start watching.');

    $('#addForm').addEventListener('submit', async e => {
      if (e.submitter && e.submitter.value !== 'add') return;
      const f = new FormData(e.target);
      try {
        const group = (f.get('group') || '').toString().trim();
        await invoke('add_host', { cfg: {
          name: (f.get('name') || '').toString().trim(),
          addr: (f.get('addr') || '').toString().trim(),
          user: '', port: 22, beszel_url: null,
          // Empty means ungrouped, never a group literally named "".
          group: group || null,
          // Empty means Linux. Sent explicitly rather than defaulted here,
          // because a Windows host created as a Linux one runs a POSIX shell
          // command against cmd.exe and fails with "the system cannot find
          // the path specified" - an error that explains nothing.
          os: (f.get('os') || '').toString(),
        }});
      } catch (err) { showError(String(err)); }
    });

    grid.addEventListener('click', async e => {
      const btn = e.target.closest('.kill');
      if (!btn) return;
      e.stopPropagation();
      const name = btn.closest('.card').dataset.name;
      try { await invoke('remove_host', { name }); }
      catch (err) { showError(String(err)); }
    });
  }

  // An empty-state message. Only ever shown when there is genuinely nothing
  // to display - never used to report an error, because it destroys the grid.
  function showEmpty(msg) {
    if (!grid.querySelector('.card')) grid.innerHTML = `<div class="empty">${esc(msg)}</div>`;
  }

  function esc(s) {
    return String(s).replace(/[<&]/g, c => ({ '<': '&lt;', '&': '&amp;' }[c]));
  }

  // Errors appear above the grid and leave existing cards alone. Previously an
  // add failure was console.warn'd when cards existed and wiped the grid when
  // they did not - invisible in one case, destructive in the other.
  let errTimer = null;
  function showError(msg) {
    console.error(msg);
    let box = document.querySelector('#errBar');
    if (!box) {
      box = document.createElement('div');
      box.id = 'errBar';
      box.className = 'errbar';
      grid.parentNode.insertBefore(box, grid);
    }
    box.innerHTML = `<b>${esc(msg)}</b><button aria-label="Dismiss">&times;</button>`;
    box.querySelector('button').onclick = () => box.remove();
    clearTimeout(errTimer);
    errTimer = setTimeout(() => box.remove(), 8000);
  }

  // ---------------------------------------------------------- SIMULATION
  function startSim() {
    $('#rateFast').addEventListener('click', () => {
      rate = 1000; $('#rateFast').setAttribute('aria-pressed', 'true');
      $('#rateSlow').setAttribute('aria-pressed', 'false');
      $('#cadNote').classList.remove('show'); start();
    });
    $('#rateSlow').addEventListener('click', () => {
      rate = 60000; $('#rateSlow').setAttribute('aria-pressed', 'true');
      $('#rateFast').setAttribute('aria-pressed', 'false');
      $('#cadNote').classList.add('show'); start();
    });

    $('#addForm').addEventListener('submit', e => {
      if (e.submitter && e.submitter.value !== 'add') return;
      const f = new FormData(e.target);
      const n = (f.get('name') || '').toString().trim();
      if (!n) return;
      hosts.push(mk(n, 'Linux', +f.get('cores'), 16, null, 2 + Math.random() * 8));
      build(); tally(); paint();
    });

    grid.addEventListener('click', e => {
      const btn = e.target.closest('.kill');
      if (!btn) return;
      e.stopPropagation();
      const name = btn.closest('.card').dataset.name;
      hosts = hosts.filter(h => h.name !== name);
      build(); tally(); paint();
    });

    build(); tally();
    for (let i = 0; i < HIST; i++) hosts.forEach(step);
    paint();
    start();
  }

  // Nothing may fail silently.
  //
  // Twice now a dead UI has cost a debugging round trip: first a CSP nonce
  // blocking the inline script, then Tauri's ACL denying core:event:listen.
  // Both presented identically - a window that renders and does nothing.
  // An unhandled rejection inside startLive() is exactly that failure mode,
  // so it now gets shown on the page instead of only in a console nobody
  // has open.
  function fatal(what, err) {
    const msg = err && err.message ? err.message : String(err);
    console.error(what, err);
    grid.innerHTML = `<div class="empty startup-error">
      <b>${what}</b>
      <code>${msg.replace(/[<&]/g, c => ({'<':'&lt;','&':'&amp;'}[c]))}</code>
      <span>Right-click &rarr; Inspect &rarr; Console for the full trace.</span>
    </div>`;
  }

  addEventListener('unhandledrejection', e => fatal('Startup failed', e.reason));
  addEventListener('error', e => fatal('Script error', e.error || e.message));

  if (LIVE) {
    startLive().catch(err => fatal('Could not start live monitoring', err));
  } else {
    startSim();
  }
})();
