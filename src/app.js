// Tuxtop frontend logic.
//
// Loaded as an external file, never inline. Tauri injects a nonce into the
// page's script-src, and per the CSP spec a nonce makes the browser ignore
// 'unsafe-inline' - so an inline <script> is silently blocked and the whole
// UI goes dead with no visible error. External files are covered by
// script-src 'self' and work under any CSP.
//
(() => {
  const $ = s => document.querySelector(s);
  const grid = $('#grid');
  const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;

  // ---- model: real hosts from the tailnet, real core counts ----
  let seq = 0;
  const mk = (name, distro, cores, ramGB, gpu, base) => ({
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
    { view: 'hosts', sort: 'manual', metric: 'cores' },
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

  const band = v => v >= 90 ? 'crit' : v >= 75 ? 'warn' : 'cool';
  const gb = v => v.toFixed(1);

  // ---- render skeleton ----
  /// Hosts in display order. 'manual' is whatever order the backend holds,
  /// which is what dragging persists -- so sorting is a view over it, never a
  /// mutation of it.
  function ordered() {
    const list = hosts.slice();
    if (prefs.sort === 'load') {
      // In the fleet view "busiest" means the metric on screen, not always CPU.
      const m = prefs.view === 'all' ? metric() : METRICS.cpu;
      return list.sort((a, b) => (m.scalar(b) || 0) - (m.scalar(a) || 0));
    }
    if (prefs.sort === 'name') {
      return list.sort((a, b) => a.name.localeCompare(b.name));
    }
    return list;
  }

  const last = a => (a.length ? a[a.length - 1] : 0);

  function bps(v) {
    const U = ['B', 'KB', 'MB', 'GB'];
    let n = v || 0, i = 0;
    while (n >= 1024 && i < U.length - 1) { n /= 1024; i++; }
    return `${i ? n.toFixed(1) : Math.round(n)} ${U[i]}/s`;
  }

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
      vector: h => h.core || [],
      scalar: h => last(h.hist.cpu),
      fmt: v => Math.round(v) + '%',
      has: h => (h.cores || 0) > 0,
    },
    cpu: {
      label: 'CPU', shape: 'scalar', scale: 'absolute', max: 100,
      scalar: h => last(h.hist.cpu),
      fmt: v => Math.round(v) + '%',
    },
    mem: {
      label: 'Memory', shape: 'scalar', scale: 'absolute', max: 100,
      scalar: h => (h.ramGB ? h.ram / h.ramGB * 100 : 0),
      fmt: v => Math.round(v) + '%',
      sub: h => `${gb(h.ram)} / ${gb(h.ramGB)} GB`,
    },
    temp: {
      // Absolute, not log: degrees are already a bounded, directly comparable
      // scale. 100C is the conventional throttle point, so the bar reads as
      // "fraction of the way to too hot".
      label: 'CPU temp', shape: 'scalar', scale: 'absolute', max: 100,
      scalar: h => (h.temp ?? null),
      fmt: v => Math.round(v) + '\u00b0C',
      has: h => h.temp !== null && h.temp !== undefined,
    },
    disk: {
      label: 'Disk I/O', shape: 'scalar', scale: 'log', floor: 1e6, decades: 4,
      scalar: h => h.dio || 0, fmt: bps,
    },
    net: {
      label: 'Network', shape: 'scalar', scale: 'log', floor: 1e6, decades: 4,
      scalar: h => h.net || 0, fmt: bps,
    },
    load: {
      label: 'Load avg', shape: 'scalar', scale: 'log', floor: 1, decades: 3,
      scalar: h => (h.load ? h.load[0] : 0), fmt: v => v.toFixed(2),
    },
    // GPU needs nvidia-smi in the sampler loop, which is Phase 6. Until a
    // host actually reports it these stay out of the picker entirely: a
    // metric that renders 0% everywhere because nothing collects it is a
    // confidently wrong number, which is the one thing this app must not do.
    gpu: {
      label: 'GPU load', shape: 'scalar', scale: 'absolute', max: 100,
      scalar: h => (h.gpu ? h.gpuU || 0 : null), fmt: v => Math.round(v) + '%',
      has: h => !!h.gpu,
    },
    gpumem: {
      label: 'GPU memory', shape: 'scalar', scale: 'absolute', max: 100,
      scalar: h => (h.gpu && h.gpuTotal ? h.gpuUsed / h.gpuTotal * 100 : null),
      fmt: v => Math.round(v) + '%',
      sub: h => (h.gpuTotal ? `${Math.round(h.gpuUsed)} / ${Math.round(h.gpuTotal)} MB` : ''),
      has: h => !!(h.gpu && h.gpuTotal),
    },
  };

  /// Map a value to 0..1 for the given metric, using a fleet-wide peak so
  /// bars are comparable between hosts.
  ///
  /// Log metrics span a fixed window of decades below the fleet peak rather
  /// than starting at zero. Anchoring the scale at 1 byte crushed everything
  /// above a megabyte into the top third - a 600x difference rendered as 69%
  /// against 100%. Four decades gives the range real visual separation, and
  /// anything quieter than that reads as the negligible traffic it is.
  function logWindow(m, peak) {
    const top = Math.max(peak || 0, m.floor || 1);
    return { top, bottom: top / Math.pow(10, m.decades || 4) };
  }

  function normalise(m, v, peak) {
    if (m.scale === 'absolute') return Math.min(1, (v || 0) / (m.max || 100));
    const { top, bottom } = logWindow(m, peak);
    if (!v || v <= bottom) return 0;
    return Math.min(1, Math.log10(v / bottom) / Math.log10(top / bottom));
  }

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
    grid.classList.add('all-mode');
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

    ordered().forEach(h => {
      const n = m.vector(h).length || h.cores || 0;
      const cols = Math.max(1, Math.min(n || 1, maxCols));
      const sec = hostBlock(h, n ? `${n} ${n === 1 ? 'core' : 'cores'}` : '? cores');

      // Fixed tracks, not 1fr: stretching tiles to fill a block wider than
      // its cores need would break the one-size rule the whole view rests on.
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
        t.title = `${h.name} core ${i}`;
        if (px >= 30) t.innerHTML = '<span class="pc"></span>';
        wrap.appendChild(t);
      }
      grid.appendChild(sec);
    });
  }

  /// Scalar metrics: one comparable bar per host.
  function buildScalar(m) {
    const wrap = document.createElement('div');
    wrap.className = 'fleetbars';
    ordered().forEach(h => {
      const row = document.createElement('div');
      row.className = 'fbar';
      row.dataset.name = h.name;
      row.innerHTML = `
        <span class="dot"></span>
        <span class="fbar-name">${esc(h.name)}</span>
        <span class="fbar-track"><span class="fbar-fill"></span></span>
        <span class="fbar-val" data-val></span>
        <span class="fbar-sub" data-sub></span>`;
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
    return sec;
  }

  function paintFleet() {
    const m = metric();
    if (grid.dataset.shape !== m.shape) { build(); return; }
    return m.shape === 'vector' ? paintVector(m) : paintScalar(m);
  }

  function paintVector(m) {
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
    const peak = hosts.reduce((a, h) => Math.max(a, m.scalar(h) ?? 0), 0);

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
    }
  }

  const dotClass = h =>
    'dot' + (h.fault ? ' warnstate' : (LIVE && !h.seen ? ' pending' : ''));

  function build() {
    if (prefs.view === 'all') return buildFleet();
    document.body.dataset.bands = 'on';
    grid.classList.remove('all-mode');
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
    ordered().forEach(h => {
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
          <div class="m"><span class="k">Disk</span><span class="v" data-dio></span></div>
          <div class="m"><span class="k">Net</span><span class="v" data-net></span></div>
          <div class="m" data-temp-chip hidden><span class="k">Temp</span><span class="v" data-temp></span></div>
          ${h.gpu ? '<div class="m"><span class="k">GPU</span><span class="v" data-gpu></span></div>' : ''}
        </div>
        <button class="card-toggle" aria-expanded="false">
          <svg width="10" height="10" viewBox="0 0 10 10"><path d="M1 3.5L5 7l4-3.5" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round"/></svg>
          <span class="tlabel">Per-core detail</span>
        </button>
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

      // Hidden rather than zeroed when a host exposes no CPU sensor, which is
      // normal on a VM. A 0 would read as an implausibly cold CPU.
      const tchip = el.querySelector('[data-temp-chip]');
      if (tchip) {
        const hasTemp = h.temp !== null && h.temp !== undefined;
        tchip.hidden = !hasTemp;
        if (hasTemp) {
          el.querySelector('[data-temp]').textContent = Math.round(h.temp) + '\u00b0C';
        }
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
    $('#ncores').textContent = hosts.reduce((a, h) => a + (h.cores || 0), 0);
    $('#nup').textContent = hosts.filter(h => !h.fault && (!LIVE || h.seen)).length;
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

  function refreshMetricOptions() {
    const avail = availableMetrics();
    const ids = avail.map(([id]) => id);
    const current = ids.join(',');
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
    if (reporting && !ids.includes(prefs.metric)) {
      prefs.metric = ids[0] || 'cores';
      savePrefs();
    }
    msel.value = prefs.metric;
  }
  refreshMetricOptions();

  function setView(v) {
    prefs.view = v; savePrefs();
    $('#viewHosts').setAttribute('aria-pressed', String(v === 'hosts'));
    $('#viewAll').setAttribute('aria-pressed', String(v === 'all'));
    $('#metricWrap').hidden = v !== 'all';
    build(); paint();
  }

  msel.addEventListener('change', e => {
    prefs.metric = e.target.value; savePrefs();
    build(); paint();
  });
  $('#viewHosts').addEventListener('click', () => setView('hosts'));
  $('#viewAll').addEventListener('click', () => setView('all'));

  $('#sortSel').value = prefs.sort;
  $('#sortSel').addEventListener('change', e => {
    prefs.sort = e.target.value; savePrefs();
    build(); paint();
  });

  // Restore the remembered view before the first render.
  $('#viewHosts').setAttribute('aria-pressed', String(prefs.view === 'hosts'));
  $('#viewAll').setAttribute('aria-pressed', String(prefs.view === 'all'));
  $('#metricWrap').hidden = prefs.view !== 'all';

  // Tile sizing in all-cores mode depends on window width.
  addEventListener('resize', () => { if (prefs.view === 'all') build(); });

  const dlg = $('#addDlg');
  $('#addBtn').addEventListener('click', () => {
    // <form method="dialog"> does not reset on close, so reopening kept the
    // previous host's details - submit again and you get a duplicate-name
    // rejection for a host you thought you were adding fresh.
    $('#addForm').reset();
    dlg.showModal();
    $('#f-name').focus();
    $('#f-name').select();
  });
  addEventListener('resize', () => paint());

  // Reveal highlight. Windows' own Fluent surfaces light up under the
  // cursor; matching that makes the app read as more native, not less.
  grid.addEventListener('pointermove', e => {
    const panel = e.target.closest('.card, .hostblock');
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
      if (h.temp !== null) push(h.hist.temp, h.temp);
      if (s.gpu) { h.gpu = s.gpu.name; h.gpuU = s.gpu.util_pct; }
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
      // Reconcile both ways. Filtering alone only ever removed, so a newly
      // added host got no card until its first sample arrived - and an
      // unreachable host never sends one, so it stayed invisible for the
      // full ssh connect timeout or forever. Order follows the backend list.
      const byName = new Map(hosts.map(h => [h.name, h]));
      hosts = list.map(c => byName.get(c.name) || mk(c.name, '', 0, 0, null, 0));
      build(); paint(); tally();
    });

    // Seed cards from hosts.toml so they exist before the first sample
    // lands -- otherwise the window is empty for a second on every launch.
    try {
      for (const cfg of await invoke('list_hosts')) ensure(cfg.name, 0);
    } catch (e) {
      showError(`Could not read hosts.toml: ${e}`);
      return;
    }

    build(); paint(); tally();
    if (!hosts.length) showEmpty('No hosts yet. Add one to start watching.');

    $('#addForm').addEventListener('submit', async e => {
      if (e.submitter && e.submitter.value !== 'add') return;
      const f = new FormData(e.target);
      try {
        await invoke('add_host', { cfg: {
          name: (f.get('name') || '').toString().trim(),
          addr: (f.get('addr') || '').toString().trim(),
          user: '', port: 22, beszel_url: null,
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
