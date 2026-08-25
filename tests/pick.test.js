// Picking the reading that matters out of the many a host reports.
//
// Choosing wrongly here produces a confident wrong number, which is the exact
// failure this project exists in response to.

const { test } = require('node:test');
const assert = require('node:assert');
const { fullestFs, fsPct, sensorName, sensorMetric, hottestSensor,
        machine, machineLabel, stealIsMeaningful } = require('../src/pick.js');

const HOST = {
  fs: [
    { mount: '/',     used_kb: 8_400_000,  total_kb: 100_000_000 },
    { mount: '/home', used_kb: 90_000_000, total_kb: 100_000_000 },
    { mount: '/boot', used_kb: 300_000,    total_kb: 1_000_000 },
  ],
};

test('the_fullest_mount_wins_not_the_average', () => {
  // A roomy / must not hide a full /home. An average across mounts is the
  // single most common way this reads wrong.
  assert.strictEqual(fullestFs(HOST).mount, '/home');
  assert.ok(Math.abs(fsPct(HOST) - 90) < 0.01);
});

test('a_host_with_no_filesystems_reports_null_not_zero', () => {
  // A host that has not sent a df frame yet has not said its disks are empty.
  assert.strictEqual(fullestFs({}), null);
  assert.strictEqual(fsPct({}), null);
  assert.strictEqual(fsPct({ fs: [] }), null);
});

test('a_mount_with_no_size_does_not_win_by_dividing_by_zero', () => {
  // tmpfs and friends report a zero total; treating that as 100% full would
  // pin every host's disk bar.
  const h = { fs: [{ mount: '/x', used_kb: 5, total_kb: 0 },
                   { mount: '/', used_kb: 10, total_kb: 100 }] };
  assert.strictEqual(fullestFs(h).mount, '/');
  assert.strictEqual(fsPct(h), 10);
});

test('unlabelled_sensors_are_numbered_within_their_driver', () => {
  // dove's board exposes four gigabyte_wmi inputs with no labels; naming them
  // all alike would show one reading and hide three.
  const t = { driver: 'gigabyte_wmi', label: '' };
  assert.strictEqual(sensorName(t, 0), 'gigabyte_wmi');
  assert.strictEqual(sensorName(t, 1), 'gigabyte_wmi 2');
  assert.strictEqual(sensorName(t, 3), 'gigabyte_wmi 4');
});

test('a_labelled_sensor_is_named_by_driver_and_label', () => {
  assert.strictEqual(sensorName({ driver: 'nvme', label: 'Sensor 1' }, 0), 'nvme Sensor 1');
});

test('the_sensor_series_key_matches_the_one_rust_writes', () => {
  // history_store.rs::sensor_key. If these drift, a chart asks for a series
  // nothing writes and renders "no history yet" forever - with no error.
  assert.strictEqual(sensorMetric({ driver: 'nvme', label: 'Sensor 1' }), 'temp.nvme.Sensor_1');
  assert.strictEqual(sensorMetric({ driver: 'k10temp', label: 'Tctl' }), 'temp.k10temp.Tctl');
  assert.strictEqual(sensorMetric({ driver: 'gigabyte_wmi', label: '', idx: 0 }), 'temp.gigabyte_wmi.0');
  assert.strictEqual(sensorMetric({ driver: 'gigabyte_wmi', label: '', idx: 3 }), 'temp.gigabyte_wmi.3');
});

test('the_hottest_sensor_is_whatever_is_hottest_not_the_cpu', () => {
  // On dove an NVMe runs 40 degrees above the CPU. This function must return
  // it - and callers must never label the result "CPU temperature".
  const h = { temps: [
    { driver: 'k10temp', label: 'Tctl', celsius: 31.6, name: 'k10temp Tctl' },
    { driver: 'nvme', label: 'Sensor 1', celsius: 71.9, name: 'nvme Sensor 1' },
    { driver: 'nvme', label: 'Composite', celsius: 49.9, name: 'nvme Composite' },
  ] };
  assert.strictEqual(hottestSensor(h).name, 'nvme Sensor 1');
});

test('a_host_with_no_sensors_has_no_hottest_one', () => {
  // A VM reports nothing. Zero degrees would render as an implausibly cold
  // machine rather than as an absent sensor.
  assert.strictEqual(hottestSensor({}), null);
  assert.strictEqual(hottestSensor({ temps: [] }), null);
});

// ---- what kind of machine a host is ---------------------------------------

const host = (virt, kind) => ({ facts: { virt, virt_kind: kind } });

test('bare_metal_gets_no_badge_because_that_is_the_assumption_already', () => {
  // dove, wader and coot. A chip on every card would bury the few that are
  // not hardware.
  assert.strictEqual(machine(host('none', 'vm')), 'metal');
  assert.strictEqual(machineLabel(host('none', 'vm')), '');
});

test('a_kvm_guest_is_named_as_one', () => {
  // heron is a Hetzner vServer: 4 vCPUs that are not 4 cores.
  assert.strictEqual(machine(host('kvm', 'vm')), 'vm');
  assert.strictEqual(machineLabel(host('kvm', 'vm')), 'kvm guest');
});

test('wsl_is_a_guest_even_though_systemd_reports_it_as_a_container', () => {
  // The host itself says virtkind=container - no firmware, no virtual BIOS,
  // fair by systemd's definition. Wrong for this question: WSL2 runs its own
  // kernel with its own memory, which is why owl reports 31 GB while the
  // machine it runs on has 64. A container would report its host's.
  assert.strictEqual(machine(host('wsl', 'container')), 'vm');
  assert.strictEqual(machineLabel(host('wsl', 'container')), 'wsl guest');
});

test('a_corrupt_value_is_unknown_rather_than_laundered_into_a_claim', () => {
  // "none unknown" is what a broken shell fallback produced on every
  // bare-metal host: systemd-detect-virt exits non-zero when it finds nothing
  // and still prints "none".
  assert.strictEqual(machine(host('none unknown', 'vm')), 'unknown');
  assert.strictEqual(machine(host('', '')), 'unknown');
  assert.strictEqual(machine({}), 'unknown');
});

test('an_unknown_host_gets_no_badge_but_is_not_called_metal', () => {
  // Silence is not evidence of silicon, and a badge reading "unknown" on an
  // old host is noise. It stays unlabelled and out of the physical count.
  assert.strictEqual(machineLabel(host('', '')), '');
  assert.notStrictEqual(machine(host('', '')), 'metal');
});

test('steal_is_shown_only_where_a_hypervisor_could_take_the_time', () => {
  // On bare metal it is structurally zero, so a figure there implies a
  // measurement that does not exist. Unknown errs toward showing it: a hidden
  // real number is worse than a shown zero.
  assert.strictEqual(stealIsMeaningful(host('none', 'vm')), false);
  assert.strictEqual(stealIsMeaningful(host('kvm', 'vm')), true);
  assert.strictEqual(stealIsMeaningful(host('wsl', 'container')), true);
  assert.strictEqual(stealIsMeaningful(host('', '')), true);
});

test('mount_rows_list_every_filesystem_fullest_first', () => {
  const { mountRows } = require('../src/pick.js');
  // coot really does carry thirteen after bind mounts are collapsed. The card
  // shows one of them; this is the rest.
  const h = { fs: [
    { mount: '/', total_kb: 1000, used_kb: 100 },
    { mount: '/mnt/usb-data', total_kb: 1000, used_kb: 290 },
    { mount: '/rpool/pbs', total_kb: 1000, used_kb: 430 },
  ]};
  const rows = mountRows(h);
  assert.deepEqual(rows.map(r => r.mount), ['/rpool/pbs', '/mnt/usb-data', '/']);
  assert.ok(Math.abs(rows[0].pct - 43) < 0.01);
});

test('a_zero_sized_filesystem_is_not_a_percentage_of_nothing', () => {
  const { mountRows } = require('../src/pick.js');
  const rows = mountRows({ fs: [{ mount: '/x', total_kb: 0, used_kb: 0 }] });
  assert.deepEqual(rows, [], 'dividing by it would invent a number');
});

test('a_host_that_has_not_reported_filesystems_yet_has_no_rows', () => {
  const { mountRows } = require('../src/pick.js');
  // df runs on a slow cadence, so this is the normal state for the first
  // half-minute of a connection - not an error, and not an empty disk.
  assert.deepEqual(mountRows({}), []);
  assert.deepEqual(mountRows({ fs: [] }), []);
});
