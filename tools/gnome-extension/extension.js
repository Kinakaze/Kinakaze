/* exported init */
const {Clutter, Gio, GLib, St} = imports.gi;
const Main = imports.ui.main;
const DND = imports.ui.dnd;
const PanelMenu = imports.ui.panelMenu;
const PopupMenu = imports.ui.popupMenu;

class DesktopWindows {
    enable() {
        this._signals = [];
        this._state = {windows: [], workspace: 0, workspaces: 4};
        this._cancellable = new Gio.Cancellable();
        this._panel = new St.BoxLayout({vertical: true, visible: false,
            reactive: true, style: 'padding: 20px; spacing: 18px;'});
        Main.layoutManager.overviewGroup.add_child(this._panel);
        this._indicator = new PanelMenu.Button(0.0, 'Workspaces');
        this._label = new St.Label({text: 'Workspace 1', y_align: Clutter.ActorAlign.CENTER});
        this._indicator.add_child(this._label);
        const show = new PopupMenu.PopupMenuItem('Applications');
        show.connect('activate', () => Main.overview.show());
        this._indicator.menu.addMenuItem(show);
        for (let i = 0; i < 4; i++) {
            const item = new PopupMenu.PopupMenuItem(`Workspace ${i + 1}`);
            item.connect('activate', () => this._workspace(i));
            this._indicator.menu.addMenuItem(item);
        }
        Main.panel.addToStatusArea('kinakaze-workspaces', this._indicator, 0, 'left');
        this._connect(Main.overview, 'showing', () => {
            this._send({op: 'overview', enabled: true});
            this._render();
        });
        this._connect(Main.overview, 'hiding', () => {
            this._panel.hide();
            this._send({op: 'overview', enabled: false});
        });
        this._connect(global.workspace_manager, 'active-workspace-changed', () => {
            this._send({op: 'workspace', index: global.workspace_manager.get_active_workspace_index()});
        });
        this._connect(Main.layoutManager, 'monitors-changed', () => this._render());
        this._open();
    }

    _connect(object, signal, callback) {
        this._signals.push([object, object.connect(signal, callback)]);
    }

    _open() {
        const raw = GLib.getenv('KINAKAZE_DESKTOP_BRIDGE');
        if (!raw) return;
        const config = JSON.parse(raw);
        this._client = new Gio.SocketClient();
        this._client.connect_to_host_async('127.0.0.1', config.port, this._cancellable, (client, result) => {
            try {
                this._connection = client.connect_to_host_finish(result);
                this._output = new Gio.DataOutputStream({base_stream: this._connection.get_output_stream()});
                this._input = new Gio.DataInputStream({base_stream: this._connection.get_input_stream()});
                this._send({token: config.token});
                this._send({op: 'workspace', index: global.workspace_manager.get_active_workspace_index()});
                this._send({op: 'overview', enabled: Main.overview.visible});
                this._read();
                log('Kinakaze desktop window bridge connected');
            } catch (error) {
                if (!this._cancellable.is_cancelled()) logError(error, 'Desktop window bridge connection');
            }
        });
    }

    _send(message) {
        if (!this._output) return;
        try { this._output.put_string(JSON.stringify(message) + '\n', this._cancellable); }
        catch (error) { if (!this._cancellable.is_cancelled()) logError(error, 'Desktop window command'); }
    }

    _read() {
        this._input.read_line_async(GLib.PRIORITY_DEFAULT, this._cancellable, (input, result) => {
            try {
                const [line] = input.read_line_finish_utf8(result);
                if (line === null) return;
                const state = JSON.parse(line);
                if (state.type === 'state') {
                    if (this._pendingWorkspace !== undefined) {
                        if (state.workspace === this._pendingWorkspace) this._pendingWorkspace = undefined;
                        else state.workspace = this._pendingWorkspace;
                    }
                    this._state = state;
                    this._label.text = `Workspace ${state.workspace + 1}`;
                    // Shell owns workspace selection. A queued older snapshot
                    // must not activate its workspace and echo a rollback to
                    // the bridge while a newer click is still in flight.
                    try { this._render(); } catch (error) { logError(error, 'Desktop overview rendering'); }
                } else if (state.type === 'error') log(`Desktop window bridge: ${state.message}`);
                this._read();
            } catch (error) {
                if (!this._cancellable.is_cancelled()) logError(error, 'Desktop window bridge read');
            }
        });
    }

    _workspace(index) {
        this._pendingWorkspace = index;
        this._state.workspace = index;
        this._label.text = `Workspace ${index + 1}`;
        this._render();
        // Native windows are managed by the bridge. Mutter has no corresponding
        // MetaWindows and switching its empty workspace adds a second animation
        // and an X-server round trip without moving any application.
        this._send({op: 'workspace', index});
    }

    _button(text, callback, style = '') {
        const button = new St.Button({label: text, can_focus: true, reactive: true,
            style: `padding: 9px 14px; border-radius: 10px; background-color: #3b3b43; color: white; ${style}`});
        button.connect('clicked', callback);
        return button;
    }

    _render() {
        if (this._redraw || !this._panel) return;
        // Never destroy a button/drag source from inside its own event signal.
        // Coalesce socket deltas and input actions into one idle redraw.
        this._redraw = GLib.idle_add(GLib.PRIORITY_DEFAULT_IDLE, () => {
            this._redraw = 0;
            try { this._draw(); } catch (error) { logError(error, 'Desktop overview rendering'); }
            return GLib.SOURCE_REMOVE;
        });
    }

    _draw() {
        this._send({op: 'thumbnails', items: []});
        if (!this._panel || !Main.overview.visible || this._dragging) return;
        const thumbnails = Main.overview._overview.controls._thumbnailsBox;
        if (thumbnails) thumbnails.hide();
        this._panel.destroy_all_children();
        const monitor = Main.layoutManager.primaryMonitor;
        if (!monitor) return;
        const width = Math.min(1320, monitor.width - 100);
        this._panel.set_position(monitor.x + (monitor.width-width)/2, monitor.y + 170);
        this._panel.set_size(width, Math.max(260, monitor.height-350));
        const tabs = new St.BoxLayout({style: 'spacing: 12px;'});
        for (let index = 0; index < this._state.workspaces; index++) {
            const count = this._state.windows.filter(window => window.workspace === index).length;
            const tab = this._button(`Workspace ${index+1} · ${count}`, () => this._workspace(index),
                index === this._state.workspace ? 'background-color: #3584e4;' : '');
            tab._delegate = {
                handleDragOver: source => source.desktopWindow ? DND.DragMotionResult.MOVE_DROP : DND.DragMotionResult.NO_DROP,
                acceptDrop: source => {
                    if (!source.desktopWindow) return false;
                    this._send({op: 'move', id: source.desktopWindow.id, index});
                    return true;
                },
            };
            tabs.add_child(tab);
        }
        this._panel.add_child(tabs);
        const scroll = new St.ScrollView({x_expand: true, y_expand: true});
        const layout = new Clutter.GridLayout({orientation: Clutter.Orientation.HORIZONTAL, column_spacing: 18, row_spacing: 18});
        const grid = new St.Widget({layout_manager: layout, x_expand: true});
        const content = new St.BoxLayout({vertical: true});
        content.add_child(grid);scroll.add_actor(content);this._panel.add_child(scroll);
        const windows = this._state.windows.filter(window => window.workspace === this._state.workspace);
        const columns = Math.max(1, Math.floor((width-40)/300));
        const previews = [];
        windows.forEach((window, position) => {
            const card = new St.BoxLayout({vertical: true, style: 'spacing: 8px; padding: 12px; background-color: #303038; border-radius: 16px;', width: 280});
            const activate = new St.Button({reactive: true, can_focus: true});
            const face = new St.BoxLayout({vertical: true, style: 'spacing: 8px;'});
            if (this._state.nativePreviews) {
                const preview = new St.Widget({width: 256, height: 160});
                face.add_child(preview);
                previews.push([window.id, preview]);
            } else if (window.preview) {
                const preview = new St.Icon({gicon: new Gio.FileIcon({file: Gio.File.new_for_path(window.preview)}),
                    icon_size: 256, width: 256, height: 160});
                face.add_child(preview);
            } else face.add_child(new St.Icon({icon_name: 'application-x-executable-symbolic', icon_size: 72, height: 160}));
            face.add_child(new St.Label({text: window.title, style: 'color: white; font-size: 15px;'}));
            activate.set_child(face);
            activate.connect('clicked', () => {
                this._workspace(window.workspace);
                this._send({op: 'activate', id: window.id});
                Main.overview.hide();
            });
            activate._delegate = {desktopWindow: window,
                getDragActor: () => new Clutter.Clone({source: face}),
                getDragActorSource: () => face};
            const drag = DND.makeDraggable(activate);
            drag.connect('drag-begin', () => {this._dragging = true;this._send({op: 'thumbnails', items: []});});
            drag.connect('drag-end', () => {
                this._dragging = false;
                // DND releases its pointer grab after emitting drag-end.
                // Keep the source actor alive until that cleanup finishes.
                this._render();
            });
            card.add_child(activate);
            const actions = new St.BoxLayout({style: 'spacing: 8px;'});
            actions.add_child(this._button('Move →', () => this._send({op: 'move',id: window.id,index: (window.workspace+1)%this._state.workspaces})));
            actions.add_child(this._button('×', () => this._send({op: 'close',id: window.id})));
            card.add_child(actions);
            layout.attach(card, position%columns, Math.floor(position/columns), 1, 1);
        });
        if (!windows.length) content.add_child(new St.Label({text: 'No windows in this workspace', style: 'padding: 40px; color: #dddddd;'}));
        this._panel.show();
        if (this._state.nativePreviews) {
            const publish = () => {
                if (!this._panel || !this._panel.visible || this._dragging) return;
                const items = previews.filter(([,actor]) => actor.get_parent()).map(([id, actor]) => {
                    const [x,y] = actor.get_transformed_position();
                    const [w,h] = actor.get_transformed_size();
                    return {id,rect:[x,y,x+w,y+h]};
                });
                this._send({op: 'thumbnails',items});
            };
            grid.connect('allocation-changed', publish);
            scroll.vscroll.adjustment.connect('notify::value', publish);
        }
    }

    disable() {
        this._send({op: 'release'});
        this._cancellable.cancel();
        if (this._redraw) GLib.source_remove(this._redraw);
        for (const [object, id] of this._signals) object.disconnect(id);
        this._signals = [];
        if (this._connection) this._connection.close(null);
        this._output = null;
        this._panel.destroy();this._panel = null;
        const thumbnails = Main.overview._overview.controls._thumbnailsBox;
        if (thumbnails) thumbnails.show();
        this._indicator.destroy();
    }
}

function init() { return new DesktopWindows(); }
