"""Change visible pixels only after GTK receives a real pointer motion."""
import gi

gi.require_version('Gtk', '4.0')
from gi.repository import Gtk, GLib

loop = GLib.MainLoop()
window = Gtk.Window(title='Kinakaze pointer latency probe')
window.set_default_size(600, 400)
window.set_titlebar(Gtk.HeaderBar())
area = Gtk.DrawingArea()
right = False


def motion(controller, x, y):
    global right
    state = x >= area.get_width() / 2
    if state != right:
        right = state
        area.queue_draw()


def draw(widget, context, width, height):
    context.set_source_rgb(1 if right else 0, 0 if right else 1, 0)
    context.paint()


controller = Gtk.EventControllerMotion()
controller.connect('motion', motion)
area.add_controller(controller)
area.set_draw_func(draw)
window.set_child(area)
window.connect('close-request', lambda *_: (loop.quit(), False)[1])
window.present()
GLib.timeout_add_seconds(40, lambda: (loop.quit(), False)[1])
loop.run()
window.destroy()
