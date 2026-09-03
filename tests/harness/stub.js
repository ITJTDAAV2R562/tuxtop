// Fake Tauri backend mirroring the Rust logic in hostlist.rs + supervisor.rs,
// so the real app.js live path can be driven in a browser.
(() => {
  const listeners = {};
  // The fleet, from tests/harness/fleet.json via scripts/harness.py, so the
  // harness tests the shape that actually exists. Falls back to a small fleet
  // when opened directly without the generator.
  const FLEET = (typeof window !== 'undefined' && window.__FLEET__) || [
    { name: 'dove', group: 'physical', cores: 32, virt: 'none' },
    { name: 'heron', group: 'VM', cores: 4, virt: 'kvm' },
  ];
  const CORES = Object.fromEntries(FLEET.map(h => [h.name, h.cores]));
  const VIRT = Object.fromEntries(FLEET.map(h => [h.name, h]));
  let hosts = FLEET.map(h => ({
    name: h.name, addr: h.name, user: '', port: 22, beszel_url: null,
    group: h.group || null, os: h.os || '', paused: false,
  }));

  /// Filesystems per host, where they differ from a single root.
  ///
  /// `df` runs on a slow cadence, so these arrive with every frame here where
  /// the real sampler sends them every thirtieth - the shapes the UI must
  /// handle are the same either way.
  const FS = {
    coot: [
      { mount: '/', total_kb: 1416 * 1048576, used_kb: 9 * 1048576 },
      { mount: '/rpool/pbs', total_kb: 400 * 1048576, used_kb: 173 * 1048576 },
      { mount: '/mnt/usb-data', total_kb: 1024 * 1048576, used_kb: 298 * 1048576 },
      { mount: '/mnt/pbs-mirror', total_kb: 2658 * 1048576, used_kb: 165 * 1048576 },
      { mount: '/rpool/data/subvol-103-disk-0', total_kb: 8 * 1048576, used_kb: 1.4 * 1048576 },
      { mount: '/var/lib/vz', total_kb: 1416 * 1048576, used_kb: 9 * 1048576 },
      { mount: '/etc/pve', total_kb: 0.1 * 1048576, used_kb: 0 },
    ],
    wader: [
      { mount: '/', total_kb: 466 * 1048576, used_kb: 229 * 1048576 },
      { mount: '/boot', total_kb: 1 * 1048576, used_kb: 0.6 * 1048576 },
      { mount: '/mnt/hdd_root', total_kb: 914 * 1048576, used_kb: 228 * 1048576 },
    ],
    serin: [
      { mount: '/', total_kb: 200 * 1048576, used_kb: 2 * 1048576 },
      { mount: '/hdd', total_kb: 3600 * 1048576, used_kb: 12 * 1048576 },
    ],
  };

  const settings = { interval_ms: 1000, history_cap_mb: 256, always_on_top: false,
                     update_check: true };

  /// What `plugin:updater|check` answers with.
  ///
  /// Null means up to date, which is the default so the suite is not staring
  /// at an update banner in every unrelated test. A spec that wants the banner
  /// sets window.__stubUpdate before loading the page.
  const stubUpdate = () => (window.__stubUpdate ?? null);
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
    // Guests get real steal; metal gets zero, as the kernel would report.
    cpu_breakdown: {
      user: 8 + Math.random() * 4, system: 3 + Math.random() * 2,
      iowait: Math.random() * 2,
      steal: ((VIRT[name] || {}).virt || 'none') !== 'none' ? 2 + Math.random() * 3 : 0,
    },
    // Mirrors the real fleet: mostly bare metal, one KVM guest, one WSL guest
    // that systemd reports as a container.
    facts: {
      kernel: 'Linux 6.12', os: 'Debian GNU/Linux 13', arch: 'x86_64',
      cpu_model: 'AMD Ryzen 9 5950X 16-Core Processor',
      virt: (VIRT[name] || {}).virt || 'none',
      // systemd really does report WSL as a container; the fleet profile
      // carries that so the classification is exercised, not assumed.
      virt_kind: (VIRT[name] || {}).virt_kind || 'vm',
    },
    // Mirrors dove: the NVMe runs far hotter than the CPU, and the board
    // exposes several unlabelled inputs. Both are the cases the UI has to get
    // right. A host with no CPU sensor has no sensors at all, like a VM.
    temps: (name.length % 3 === 0) ? [] : [
      { driver: 'k10temp', label: 'Tctl', kind: 'cpu',
        celsius: 34 + (name.length * 7) % 45 },
      { driver: 'k10temp', label: 'Tccd1', kind: 'cpu',
        celsius: 36 + (name.length * 5) % 40 },
      { driver: 'nvme', label: 'Composite', kind: 'drive', celsius: 48 + Math.random() * 4 },
      { driver: 'nvme', label: 'Sensor 1', kind: 'drive', celsius: 68 + Math.random() * 6 },
      { driver: 'gigabyte_wmi', label: '', kind: 'board', celsius: 33 + Math.random() * 3 },
      { driver: 'gigabyte_wmi', label: '', kind: 'board', celsius: 36 + Math.random() * 3 },
    ],
    // Mirrors the real spread, which is the point of the harness: most hosts
    // carry one or two filesystems and one carries thirteen. A stub where
    // every host looked alike would never exercise the disclosure, and the
    // fleet view would look finished while the case that needed it - coot's
    // ZFS datasets - had never been drawn.
    filesystems: FS[name] || [{ mount: '/', total_kb: 240 * 1048576, used_kb: 31 * 1048576 }],
  });

  function invokeHistory(args) {
    const { metric, fromSecsAgo, maxPoints, host } = args;
    const now = Math.floor(Date.now() / 1000);
    const n = Math.min(maxPoints || 200, 240);
    // Anything on a 0-100 scale. `temp.*` are the per-sensor series and
    // `hottest` the fleet metric; without them here they fell through to the
    // byte-rate branch and charted 4,500,950 degrees.
    const pct = ['cpu','mem','temp','gpu','gpumem','hottest','fs','swap'].includes(metric)
      || String(metric).startsWith('core.')
      || String(metric).startsWith('temp.');
    const load = metric === 'load';
    const degrees = metric === 'hottest' || String(metric).startsWith('temp.') || metric === 'temp';
    const base = load ? 0.6 : degrees ? (metric === 'hottest' ? 68 : 42) : pct ? 18 : 4e6;
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
      /// Tauri's IPC channel, as much of it as the updater's progress
      /// reporting uses. Real one lives in @tauri-apps/api/core.
      Channel: class { constructor() { this.onmessage = () => {}; } },
      invoke: async (cmd, args) => {
        // Plugin IPC, same names the real plugins register.
        if (cmd === 'plugin:updater|check') return stubUpdate();
        if (cmd === 'plugin:updater|download_and_install') {
          window.__stubInstalled = true;
          return null;
        }
        if (cmd === 'plugin:opener|open_url') {
          window.__stubOpened = args.url;
          return null;
        }
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
        if (cmd === 'query_history_fleet') {
          // Mirrors Service::query_history_fleet: every configured host is a
          // key, including ones with no history, so the caller can tell "not
          // reporting" from "not configured".
          const out = {};
          for (const h of hosts) out[h.name] = invokeHistory({ ...args, host: h.name });
          return out;
        }
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
          if (h) h.interval_ms = args.intervalMs ?? null;
          emit('tuxtop://hosts-changed', structuredClone(hosts));
          return structuredClone(hosts);
        }
        if (cmd === 'traffic_stats') {
          return hosts.map(h => ({
            host: h.name, interval_ms: h.interval_ms ?? settings.interval_ms,
            bytes_total: 7300 * 60, frames_total: 60, last_frame_bytes: 7300,
          }));
        }
        if (cmd === 'set_host_paused') {
          const h = hosts.find(x => x.name === args.name);
          if (!h) throw `no host named ${args.name}`;
          h.paused = !!args.paused;
          emit('tuxtop://hosts-changed', structuredClone(hosts));
          return structuredClone(hosts);
        }
        if (cmd === 'set_host_group') {
          const h = hosts.find(x => x.name === args.name);
          if (!h) throw `no host named ${args.name}`;
          h.group = (args.group || '').trim() || null;
          emit('tuxtop://hosts-changed', structuredClone(hosts));
          return structuredClone(hosts);
        }
        if (cmd === 'cgroup_list') {
          // Derived from the same process names the process list reports, so
          // the join between the two is actually exercised - plus two units
          // whose processes never reach the top twenty, which is the case
          // this view exists to surface.
          const out = [];
          for (const h of hosts) {
            const names = ['tailscaled','searchd','python','node','postgres',
                           'dockerd','nginx','redis-server','java'];
            names.forEach((n, i) => {
              const owner = n === 'dockerd' ? 'docker:4f2b8c1d9e0a'
                          : n === 'node' ? 'login session' : `${n}.service`;
              out.push({ host: h.name, name: owner,
                         cpu_pct: Math.max(0, 30 - i * 3 + Math.random() * 4),
                         memory_bytes: (950 - i * 90) * 1024 * 1024 * 1.35,
                         pids: 40 - i * 3 });
            });
            out.push({ host: h.name, name: 'manticore.service',
                       cpu_pct: 34.2, memory_bytes: 483102720, pids: 59 });
            out.push({ host: h.name, name: 'unattended-upgrades.service',
                       cpu_pct: 0, memory_bytes: 6291456, pids: 2 });
            // Both restart shapes: an old count with no recency, and one that
            // is climbing while you watch.
            out.push({ host: h.name, name: 'indexer-post.service',
                       cpu_pct: 1.1, memory_bytes: 87031808, pids: 3,
                       restarts: 4, restarts_since_seen: 0 });
            out.push({ host: h.name, name: 'flapper.service',
                       cpu_pct: 0.4, memory_bytes: 20971520, pids: 1,
                       restarts: 9, restarts_since_seen: 3 });
          }
          return out;
        }
        if (cmd === 'process_list') {
          const names = ['tailscaled','searchd','python','node','postgres',
                         'kworker/3:1','dockerd','nginx','redis-server','java'];
          const out = [];
          for (const h of hosts) {
            // Deliberately repetitive names: the real fleet has six
            // processes all called Runner.Listener, and the command line is
            // the only thing that tells them apart.
            const CMDS = {
              tailscaled: '/usr/sbin/tailscaled --state=/var/lib/tailscale/tailscaled.state --socket=/run/tailscale/tailscaled.sock',
              searchd: '/usr/bin/searchd --config /etc/sphinxsearch/sphinx.conf --nodetach',
              python: '/opt/venvs/transcribe/bin/python /home/sam/app/worker.py --queue default',
              node: '/usr/bin/node /srv/app/dist/server.js --port 3000',
              postgres: '/usr/lib/postgresql/16/bin/postgres -D /var/lib/postgresql/16/main',
              dockerd: '/usr/bin/dockerd -H fd:// --containerd=/run/containerd/containerd.sock',
              nginx: 'nginx: worker process',
              'redis-server': '/usr/bin/redis-server 127.0.0.1:6379',
              java: '/usr/lib/jvm/java-21/bin/java -Xmx4g -jar /srv/indexer/indexer.jar',
            };
            names.forEach((n, i) => out.push({
              host: h.name, pid: 1000 + i * 37 + h.name.length,
              cpu_pct: Math.max(0, 45 - i * 5 + Math.random() * 6),
              rss_kb: (950 - i * 90) * 1024,
              user: i % 3 ? 'root' : 'sam', comm: n,
              // Kernel threads have no command line at all.
              cmd: n.startsWith('kworker') ? '' : `${CMDS[n] || n} --host=${h.name}`,
              // Real shapes: a service, a container scope, a login session,
              // and a kernel thread with no cgroup at all.
              owner: n.startsWith('kworker') ? ''
                   : n === 'dockerd' ? 'docker:4f2b8c1d9e0a'
                   : n === 'node' ? 'login session'
                   : `${n}.service`,
              owner_kind: n.startsWith('kworker') ? 'none'
                        : n === 'dockerd' ? 'container'
                        : n === 'node' ? 'session' : 'service',
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

  // dove is pinned so the group aggregate and its worst member disagree - the
  // case the whole feature exists to render correctly.
  setInterval(() => {
    for (const h of hosts) {
      // Mirrors `Supervisor::start`: a paused host has no sampler, so it
      // emits nothing. Without this the stub would keep feeding a card the
      // UI is supposed to have blanked, and the E2E test would be asserting
      // against a backend that does not behave like the real one.
      if (h.paused) continue;
      const s = sample(h.name, CORES[h.name] || 8);
      if (h.name === 'dove') {
        s.cpu = 97; s.cores = s.cores.map(() => 90 + Math.random() * 10);
      }
      emit('tuxtop://sample', s);
    }
  }, 400);
  window.__STUB__ = { hosts: () => hosts };
})();
