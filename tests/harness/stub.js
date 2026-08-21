// Fake Tauri backend mirroring the Rust logic in hostlist.rs + supervisor.rs,
// so the real app.js live path can be driven in a browser.
(() => {
  const listeners = {};
  let hosts = [{ name: 'dove', addr: 'dove', user: '', port: 22, beszel_url: null }];
  const emit = (ev, payload) => (listeners[ev] || []).forEach(f => f({ payload }));

  const sample = (name, nCores) => ({
    host: name, cpu: Math.random() * 30,
    cores: Array.from({ length: nCores }, () => Math.random() * 40),
    mem_used_kb: 5 * 1048576, mem_total_kb: 31 * 1048576,
    net_rx_bps: 1e6, net_tx_bps: 2e5, disk_read_bps: 0, disk_write_bps: 5e5,
    gpu: null, load: [0.4, 0.3, 0.2],
  });

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
        if (cmd === 'active_hosts') return hosts.map(h => h.name);
        throw `unknown command ${cmd}`;
      },
    },
    event: {
      listen: async (ev, cb) => { (listeners[ev] ||= []).push(cb); return () => {}; },
    },
  };
  // dove reports continuously, like a healthy host.
  setInterval(() => { if (hosts.some(h => h.name === 'dove')) emit('tuxtop://sample', sample('dove', 32)); }, 400);
  window.__STUB__ = { hosts: () => hosts };
})();
