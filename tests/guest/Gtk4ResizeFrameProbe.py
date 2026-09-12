"""Animated GTK4 window for native resize/move pixel verification."""
import gi
gi.require_version('Gtk', '4.0')
from gi.repository import Gtk, GLib

loop = GLib.MainLoop()
window = Gtk.Window(title='Kinakaze GTK4 resize frame probe')
window.set_default_size(640, 440)
window.set_titlebar(Gtk.HeaderBar())
area = Gtk.DrawingArea()
frame = 0

def draw(widget, context, width, height):
    color = [(1, 0, 0), (0, 1, 0), (0, 0, 1)][frame % 3]
    context.set_source_rgb(*color)
    context.paint()
    # Moving white geometry also makes stretched or stale frames observable.
    context.set_source_rgb(1, 1, 1)
    context.rectangle(width - 70, height - 70, 35, 35)
    context.fill()

def tick(widget, clock):
    global frame
    frame += 1
    area.queue_draw()
    return True

area.set_draw_func(draw)
area.add_tick_callback(tick)
window.set_child(area)
window.connect('close-request', lambda *_: (loop.quit(), False)[1])
window.present()
print("GTK4_RENDERER", window.get_renderer().__gtype__.name, flush=True)
GLib.timeout_add_seconds(20, lambda: (loop.quit(), False)[1])
loop.run()
window.destroy()
print('GTK4_RESIZE_FRAME_PROBE_FINISHED', flush=True)
