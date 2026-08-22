// Fake Tauri backend mirroring the Rust logic in hostlist.rs + supervisor.rs,
// so the real app.js live path can be driven in a browser.
(() => {
  const listeners = {};
  let hosts = [
    { name: 'dove',  addr: 'dove',  user: '', port: 22, beszel_url: null, group: 'workstations' },
    { name: 'coot',  addr: 'coot',  user: '', port: 22, beszel_url: null, group: 'workstations' },
    { name: 'heron', addr: 'heron', user: '', port: 22, beszel_url: null, group: 'servers' },
    { name: 'wader', addr: 'wader', user: '', port: 22, beszel_url: null, group: 'servers' },
    { name: 'owl',   addr: 'owl',   user: '', port: 22, beszel_url: null, group: null },
  ];
  const settings = { interval_secs: 1, history_cap_mb: 256, always_on_top: false };
  const emit = (ev, payload) => (listeners[ev] || []).forEach(f => f({ payload }));

  const sample = (name, nCores) => ({
    host: name, cpu: Math.random() * 30,
    cores: Array.from({ length: nCores }, () => Math.random() * 40),
    mem_used_kb: 5 * 1048576, mem_total_kb: 31 * 1048576,
    net_rx_bps: 1e6, net_tx_bps: 2e5, disk_read_bps: 0, disk_write_bps: 5e5,
    gpu: null, load: [0.4, 0.3, 0.2],
    // Some hosts expose no CPU sensor - VMs typically do not - so the UI has
    // to handle both in the same fleet.
    cpu_temp_c: (name.length % 3 === 0) ? null : 34 + (name.length * 7) % 45,
  });

  function invokeHistory(args) {
    const { metric, fromSecsAgo, maxPoints, host } = args;
    const now = Math.floor(Date.now() / 1000);
    const n = Math.min(maxPoints || 200, 240);
    const pct = ['cpu','mem','temp','gpu','gpumem'].includes(metric) || String(metric).startsWith('core.');
    const load = metric === 'load';
    const base = load ? 0.6 : pct ? 18 : 4e6;
    const seed = String(metric).length * 3 + String(host || '').length * 5;
    // coot falls silent through the middle of every window. The backend skips
    // gaps rather than returning nulls, so a silent host yields *fewer*
    // points - which is exactly the case group aggregation has to survive.
    const silent = i => host === 'coot' && i > n * 0.35 && i < n * 0.6;
    return Array.from({ length: n }, (_, i) => i).filter(i => !silent(i)).map(i => {
      const wob = Math.sin((i + seed) / 9) * (load ? 0.3 : pct ? 9 : 1.6e6);
      const mean = Math.max(0, base + wob + Math.random() * (load ? 0.2 : pct ? 5 : 8e5));
      const spike = ((i + seed) % 37 === 0) ? (load ? 2.5 : pct ? 60 : 9e6) : 0;
      return {
        t: now - fromSecsAgo + Math.round(i * fromSecsAgo / n),
        min: Math.max(0, mean * 0.7),
        mean,
        max: pct ? Math.min(100, mean * 1.25 + spike) : mean * 1.25 + spike,
      };
    });
  }

  window.__TAURI__ = {
    core: {
      invoke: async (cmd, args) => {
        if (cmd === 'list_hosts') return structuredClone(hosts);
        if (cmd === 'add_host') {
          const cfg = args.cfg;
          if (!cfg.name.trim()) throw 'host needs a name';
          if (hosts.some(h => h.name === cfg.name.trim())) throw `a host named ${cfg.name} already exists`;
          hosts.push({ ...cfg, name: cfg.name.trim() });
          emit('tuxtop://hosts-changed', structuredClone(hosts));
          // The backend starts a sampler; first sample lands a moment later.
          // unreachable host: no sample at all
          return structuredClone(hosts);
        }
        if (cmd === 'remove_host') {
          hosts = hosts.filter(h => h.name !== args.name);
          emit('tuxtop://hosts-changed', structuredClone(hosts));
          return structuredClone(hosts);
        }
        if (cmd === 'reorder_hosts') {
          // Mirrors hostlist::reorder - stable, unmentioned hosts last.
          const rank = h => { const i = args.names.indexOf(h.name); return i < 0 ? 1e9 : i; };
          hosts = hosts.slice().sort((a, b) => rank(a) - rank(b));
          emit('tuxtop://hosts-changed', structuredClone(hosts));
          return structuredClone(hosts);
        }
        if (cmd === 'query_history') return invokeHistory(args);
        if (cmd === 'query_history_many') {
          const out = {};
          for (const m of args.metrics) {
            out[m] = invokeHistory({ ...args, metric: m });
          }
          return out;
        }
        // The settings dialog reads these on open; without them the whole
        // dialog errored in the harness while working fine in the app.
        if (cmd === 'get_settings') return { ...settings };
        if (cmd === 'set_settings') { Object.assign(settings, args.settings); return { ...settings }; }
        if (cmd === 'set_host_interval') {
          const h = hosts.find(x => x.name === args.name);
          if (h) h.interval_secs = args.intervalSecs ?? null;
          emit('tuxtop://hosts-changed', structuredClone(hosts));
          return structuredClone(hosts);
        }
        if (cmd === 'traffic_stats') {
          return hosts.map(h => ({
            host: h.name, interval_secs: h.interval_secs ?? settings.interval_secs,
            bytes_total: 7300 * 60, frames_total: 60, last_frame_bytes: 7300,
          }));
        }
        if (cmd === 'set_host_group') {
          const h = hosts.find(x => x.name === args.name);
          if (!h) throw `no host named ${args.name}`;
          h.group = (args.group || '').trim() || null;
          emit('tuxtop://hosts-changed', structuredClone(hosts));
          return structuredClone(hosts);
        }
        if (cmd === 'set_processes_enabled') return null;
        if (cmd === 'process_list') {
          const names = ['tailscaled','searchd','python','node','postgres',
                         'kworker/3:1','dockerd','nginx','redis-server','java'];
          const out = [];
          for (const h of hosts) {
            names.forEach((n, i) => out.push({
              host: h.name, pid: 1000 + i * 37 + h.name.length,
              cpu_pct: Math.max(0, 45 - i * 5 + Math.random() * 6),
              rss_kb: (950 - i * 90) * 1024,
              user: i % 3 ? 'root' : 'sam', comm: n,
              kernel: n.startsWith('kworker'),
            }));
          }
          return out.sort((a, b) => b.cpu_pct - a.cpu_pct || b.rss_kb - a.rss_kb);
        }
        if (cmd === 'history_usage') {
          const series = hosts.reduce((a, h) => a + 8 + (CORES[h.name] || 8), 0);
          return {
            bytes: series * 81792 * 0.31,      // partway through filling
            series,
            full_series_bytes: 81792,
            finest_secs: settings.history_cap_mb < 32 ? 10 : 1,
          };
        }
        throw `unknown command ${cmd}`;
      },
    },
    event: {
      listen: async (ev, cb) => { (listeners[ev] ||= []).push(cb); return () => {}; },
    },
  };
  // dove reports continuously, like a healthy host.
  const CORES = { dove: 32, coot: 32, heron: 4, wader: 24, owl: 16 };
  // dove is pinned so the group aggregate and its worst member disagree - the
  // case the whole feature exists to render correctly.
  setInterval(() => {
    for (const h of hosts) {
      const s = sample(h.name, CORES[h.name] || 8);
      if (h.name === 'dove') {
        s.cpu = 97; s.cores = s.cores.map(() => 90 + Math.random() * 10);
      }
      emit('tuxtop://sample', s);
    }
  }, 400);
  window.__STUB__ = { hosts: () => hosts };
})();
