"""Interactive GTK4 fixture: context menus must work after WM move/resize."""
import gi
gi.require_version('Gtk', '4.0')
from gi.repository import Gtk, Gdk, GLib

loop = GLib.MainLoop()
window = Gtk.Window(title='Kinakaze GTK grab recovery probe')
window.set_default_size(640, 440)
window.set_titlebar(Gtk.HeaderBar())
area = Gtk.DrawingArea()
area.set_draw_func(lambda widget, cr, w, h: (cr.set_source_rgb(.1, .3, .6), cr.paint()))
window.set_child(area)
menu = Gtk.Popover()
menu.set_child(Gtk.Label(label='Context menu after window operation', margin_top=20,
                        margin_bottom=20, margin_start=20, margin_end=20))
menu.set_parent(area)
gesture = Gtk.GestureClick()
gesture.set_button(0)

def pressed(controller, count, x, y):
    button = controller.get_current_button()
    print('PRESSED', button, count, round(x), round(y), flush=True)
    if button == 3:
        rect = Gdk.Rectangle()
        rect.x, rect.y, rect.width, rect.height = int(x), int(y), 1, 1
        menu.set_pointing_to(rect)
        menu.popup()
        print('CONTEXT_MENU_OPEN', flush=True)

gesture.connect('pressed', pressed)
gesture.connect('released', lambda controller, *_: print('RELEASED', controller.get_current_button(), flush=True))
area.add_controller(gesture)
window.connect('close-request', lambda *_: (loop.quit(), False)[1])
window.present()
GLib.timeout_add_seconds(45, lambda: (loop.quit(), False)[1])
loop.run()
menu.unparent()
window.destroy()
