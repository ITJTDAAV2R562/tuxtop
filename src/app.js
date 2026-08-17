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
    hist: { cpu: [], ram: [], dio: [], net: [], gpu: [] },
    net: 0.4, dio: 2, gpuU: 0
  });

  // Under Tauri the host list comes from hosts.toml and every number arrives
  // from the backend. Opened as a plain file there is no backend, so the
  // simulator below runs against these instead — that keeps this page usable
  // as the design mockup it started life as.
  const TAURI = globalThis.__TAURI__;
  const LIVE = !!TAURI;

  let hosts = LIVE ? [] : [
    mk('dove',   'Debian 13', 32, 31,  'RTX 3080', 3),
    mk('heron',  'Debian 12',  8, 16,  null,       11),
    mk('wader',  'Debian 11',  4,  8,  null,       6),
    mk('falcon', 'Ubuntu 24',  8, 32,  null,       2),
  ];

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
  function build() {
    // build() tears down the DOM, so remember which card was open. Without
    // this, adding a host or a core-count change silently collapses the
    // detail panel the user was reading.
    const wasOpen = grid.querySelector('.card.expanded')?.dataset.name;
    grid.innerHTML = '';
    if (!hosts.length) {
      grid.innerHTML = '<div class="empty">No hosts. Add one to start watching.</div>';
      return;
    }
    hosts.forEach(h => {
      const el = document.createElement('article');
      el.className = 'card';
      el.dataset.id = h.id;
      el.dataset.name = h.name;
      el.innerHTML = `
        <div class="chead">
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
      if (!h.fault) el.querySelector('.dot').className = 'dot' + (cpu > 85 ? ' warnstate' : '');
      el.querySelector('[data-ram]').textContent = gb(h.ram) + ' / ' + h.ramGB + ' GB';
      el.querySelector('[data-dio]').textContent = Math.round(h.dio) + ' MB/s';
      el.querySelector('[data-net]').textContent = h.net.toFixed(1) + ' MB/s';
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
        el.querySelector('[data-d-dio]').textContent = Math.round(h.dio) + ' MB/s';
        el.querySelector('[data-d-net]').textContent = h.net.toFixed(1) + ' MB/s';
        if (h.gpu) {
          draw(el.querySelector('[data-c="gpu"]'), h.hist.gpu, GPU, 100);
          el.querySelector('[data-d-gpu]').textContent = Math.round(h.gpuU) + '%';
        }
      }
    });
  }

  function tally() {
    $('#nhosts').textContent = hosts.length;
    $('#ncores').textContent = hosts.reduce((a, h) => a + (h.cores || 0), 0);
    $('#nup').textContent = hosts.filter(h => !h.fault).length;
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

  const dlg = $('#addDlg');
  $('#addBtn').addEventListener('click', () => dlg.showModal());
  addEventListener('resize', () => paint());

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
      h.core = s.cores;
      h.ramGB = s.mem_total_kb / 1048576;
      h.ram = s.mem_used_kb / 1048576;
      h.net = (s.net_rx_bps + s.net_tx_bps) / 1e6;
      h.dio = (s.disk_read_bps + s.disk_write_bps) / 1e6;
      h.load = s.load;
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
      hosts = hosts.filter(h => list.some(c => c.name === h.name));
      build(); paint(); tally();
    });

    // Seed cards from hosts.toml so they exist before the first sample
    // lands — otherwise the window is empty for a second on every launch.
    try {
      for (const cfg of await invoke('list_hosts')) ensure(cfg.name, 0);
    } catch (e) {
      showEmpty(`Could not read hosts.toml: ${e}`);
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
      } catch (err) { showEmpty(String(err)); }
    });

    grid.addEventListener('click', async e => {
      const btn = e.target.closest('.kill');
      if (!btn) return;
      e.stopPropagation();
      const name = btn.closest('.card').dataset.name;
      try { await invoke('remove_host', { name }); }
      catch (err) { showEmpty(String(err)); }
    });
  }

  function showEmpty(msg) {
    if (!grid.querySelector('.card')) grid.innerHTML = `<div class="empty">${msg}</div>`;
    else console.warn(msg);
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

  if (LIVE) startLive(); else startSim();
})();
