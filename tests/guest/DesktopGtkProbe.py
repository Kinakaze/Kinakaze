import ctypes as c
import time
import sys
gtk=c.CDLL('libgtk-3.so.0')
gtk.gtk_init_check.argtypes=[c.c_void_p,c.c_void_p]; gtk.gtk_init_check.restype=c.c_int
print('GTK init',flush=True)
assert gtk.gtk_init_check(None,None), 'GTK display initialization failed'
gtk.gtk_window_new.argtypes=[c.c_int]; gtk.gtk_window_new.restype=c.c_void_p
gtk.gtk_label_new.argtypes=[c.c_char_p]; gtk.gtk_label_new.restype=c.c_void_p
gtk.gtk_container_add.argtypes=[c.c_void_p,c.c_void_p]
gtk.gtk_window_set_title.argtypes=[c.c_void_p,c.c_char_p]
gtk.gtk_widget_show_all.argtypes=[c.c_void_p]
gtk.gtk_widget_destroy.argtypes=[c.c_void_p]
gtk.gtk_widget_get_realized.argtypes=[c.c_void_p]; gtk.gtk_widget_get_realized.restype=c.c_int
gtk.gtk_main_iteration_do.argtypes=[c.c_int]; gtk.gtk_main_iteration_do.restype=c.c_int
print('GTK create window',flush=True)
window=gtk.gtk_window_new(0); assert window
label=gtk.gtk_label_new(b'Kinakaze GTK real window'); assert label
gtk.gtk_window_set_title(window,b'Kinakaze GTK probe')
print('GTK show window',flush=True)
gtk.gtk_container_add(window,label); gtk.gtk_widget_show_all(window)
if '--idle-close' in sys.argv:
    # The host sends WM_CLOSE after the main loop has gone idle. A periodic
    # timeout or manual iteration here would hide a broken connection wakeup.
    objects=c.CDLL('libgobject-2.0.so.0')
    objects.g_signal_connect_data.argtypes=[c.c_void_p,c.c_char_p,c.c_void_p,c.c_void_p,c.c_void_p,c.c_int]
    deleted=[]
    callback=c.CFUNCTYPE(c.c_int,c.c_void_p,c.c_void_p,c.c_void_p)
    @callback
    def close_window(widget,event,data):
        deleted.append(True)
        gtk.gtk_main_quit()
        return 0
    objects.g_signal_connect_data(window,b'delete-event',close_window,None,None,0)
    print('GTK idle main loop',flush=True)
    gtk.gtk_main()
    assert deleted
    print('DESKTOP_GTK_IDLE_CLOSE_OK')
    raise SystemExit(0)
print('GTK iterate',flush=True)
end=time.monotonic()+0.25
while time.monotonic()<end:
 gtk.gtk_main_iteration_do(0)
 time.sleep(0.005)
assert gtk.gtk_widget_get_realized(window)
gtk.gtk_widget_destroy(window)
for _ in range(10): gtk.gtk_main_iteration_do(0)
print('DESKTOP_GTK_WINDOW_REALIZED_DESTROYED_OK')
