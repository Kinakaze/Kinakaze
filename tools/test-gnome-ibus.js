// Exercise the actual generated GJS module, with controlled IBus/systemd state.
// No graphical session, real bus, timer, or daemon is created by this test.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

let wayland = false;
const context = vm.createContext({
    imports: {
        gi: {
            Gio: { _promisify() {} }, GLib: {}, Shell: {},
            Meta: { is_wayland_compositor: () => wayland },
            IBus: { MAJOR_VERSION: 1, MINOR_VERSION: 5, MICRO_VERSION: 27, Bus: function () {} },
        },
        misc: { signals: { EventEmitter: class {} } },
        ui: { boxpointer: {}, ibusCandidatePopup: {} },
    },
});
vm.runInContext(fs.readFileSync(process.argv[2], 'utf8'), context);
const prototype = context.IBusManager.prototype;

async function check({ connected = false, systemd = false, connectWhileWaiting = false,
                       restart = false, expected }) {
    const spawns = [];
    const manager = {
        _ibus: { is_connected: () => connected },
        async _ibusSystemdServiceExists() {
            await Promise.resolve();
            if (connectWhileWaiting) connected = true;
            return systemd;
        },
        _spawn: args => spawns.push(Array.from(args)),
    };
    if (restart) await prototype.restartDaemon.call(manager, ['--xim']);
    else await prototype._queueSpawn.call(manager);
    assert.deepEqual(spawns, expected);
}

(async () => {
    await check({ connected: true, expected: [] });
    await check({ connectWhileWaiting: true, expected: [] });
    await check({ expected: [['--xim']] });
    await check({ systemd: true, expected: [] });
    await check({ connected: true, restart: true, expected: [['-r', '--xim']] });
    await check({ systemd: true, restart: true, expected: [] });
    wayland = true;
    await check({ expected: [[]] });
    console.log('GNOME IBus: 7 startup/restart cases passed');
})().catch(error => { console.error(error); process.exitCode = 1; });
