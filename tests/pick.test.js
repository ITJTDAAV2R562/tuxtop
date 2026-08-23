// Picking the reading that matters out of the many a host reports.
//
// Choosing wrongly here produces a confident wrong number, which is the exact
// failure this project exists in response to.

const { test } = require('node:test');
const assert = require('node:assert');
const { fullestFs, fsPct, sensorName, sensorMetric, hottestSensor } =
  require('../src/pick.js');

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
