#define _GNU_SOURCE
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/extensions/XInput2.h>
#include <X11/extensions/Xrandr.h>
#include <alsa/asoundlib.h>
#include <pulse/channelmap.h>
#include <pulse/volume.h>

static int errors;
static int error_handler(Display *display, XErrorEvent *event) {
    (void)display;
    assert(event->request_code == 128);
    assert(event->error_code == BadRequest || event->error_code == BadValue);
    if (event->error_code == BadValue)
        assert(XISelectEvents(display,DefaultRootWindow(display),NULL,0)==Success);
    errors++;
    return 0;
}
static void masks(Display *d, int expected) {
    int count = -1;
    XIEventMask *selection = XIGetSelectedEvents(d, DefaultRootWindow(d), &count);
    assert(count == expected);
    if (count) {
        assert(selection->deviceid == XIAllMasterDevices);
        assert(XIMaskIsSet(selection->mask, XI_RawMotion));
    }
    XFree(selection);
}
static void image_formats(Display *d) {
    for (int unit=8;unit<=32;unit*=2) for(int order=0;order<2;order++)
    for(int bits=0;bits<2;bits++) for(int width=1;width<=33;width+=8) {
        XImage image={0};
        image.width=width;image.height=2;image.depth=1;image.format=XYBitmap;
        image.bitmap_unit=unit;image.bitmap_pad=8;image.bits_per_pixel=1;
        image.byte_order=order;image.bitmap_bit_order=bits;
        assert(XInitImage(&image));
        size_t size=image.bytes_per_line*2;
        unsigned char *storage=calloc(1,size+2);assert(storage);
        storage[0]=0x5a;storage[size+1]=0xa5;image.data=(char *)storage+1;
        for(int y=0;y<2;y++) for(int x=0;x<width;x++) assert(XPutPixel(&image,x,y,(x+y)%2));
        for(int y=0;y<2;y++) for(int x=0;x<width;x++) assert(XGetPixel(&image,x,y)==(unsigned)(x+y)%2);
        assert(storage[0]==0x5a && storage[size+1]==0xa5);free(storage);
    }
    XImage bad={0};bad.width=1;bad.height=1;bad.depth=1;bad.format=XYBitmap;
    bad.bitmap_unit=32;bad.bitmap_pad=8;bad.bitmap_bit_order=MSBFirst;
    bad.byte_order=LSBFirst;bad.bytes_per_line=1;
    assert(!XInitImage(&bad));
    for(int bpp=8;bpp<=32;bpp+=8) for(int order=0;order<2;order++) {
        unsigned char storage[16]={0};XImage image={0};
        image.width=3;image.height=1;image.depth=bpp;image.format=ZPixmap;
        image.bitmap_unit=32;image.bitmap_pad=8;image.bits_per_pixel=bpp;
        image.byte_order=order;image.data=(char *)storage;
        assert(XInitImage(&image));assert(XPutPixel(&image,2,0,0x12345678));
        assert(XGetPixel(&image,2,0)==(0x12345678UL&((1UL<<bpp)-1)));
    }
    (void)d;
}
static void audio_parameters(void) {
    snd_pcm_t *pcm=NULL;assert(!snd_pcm_open(&pcm,"default",SND_PCM_STREAM_PLAYBACK,0));
    snd_pcm_chmap_t *map=snd_pcm_get_chmap(pcm);assert(map && map->channels==2);
    assert(map->pos[0]==SND_CHMAP_FL && map->pos[1]==SND_CHMAP_FR);
    char buffer[64];assert(snd_pcm_chmap_print(map,sizeof buffer,buffer)==5 && !strcmp(buffer,"FL FR"));
    assert(snd_pcm_chmap_print(map,3,buffer)<0 && buffer[2]==0);
    free(map);assert(!snd_pcm_close(pcm));
    void **hints=NULL;assert(!snd_device_name_hint(-1,"pcm",&hints));assert(hints);
    for (void **h=hints;*h;h++) {
        char *name=snd_device_name_get_hint(*h,"NAME"),*io=snd_device_name_get_hint(*h,"IOID");
        assert(name && io && !strcmp(name,"default") && !strcmp(io,"Output"));free(name);free(io);
    }
    assert(!snd_device_name_free_hint(hints));
    snd_pcm_hw_params_t *hw=NULL;assert(!snd_pcm_hw_params_malloc(&hw));
    assert(!snd_pcm_hw_params_any(NULL,hw));unsigned periods=2;int dir=0;
    assert(!snd_pcm_hw_params_set_periods_min(NULL,hw,&periods,&dir));
    assert(!snd_pcm_hw_params_set_periods_first(NULL,hw,&periods,&dir) && periods==2);
    periods=3;assert(snd_pcm_hw_params_set_periods_min(NULL,hw,&periods,&dir)<0);
    snd_pcm_hw_params_free(hw);
    for(int def=0;def<PA_CHANNEL_MAP_DEF_MAX;def++) for(unsigned n=1;n<=32;n++) {
        pa_channel_map value,parsed;char text[1024];
        assert(pa_channel_map_init_extend(&value,n,def) && pa_channel_map_valid(&value));
        assert(value.channels==n);pa_channel_map_snprint(text,sizeof text,&value);
        assert(pa_channel_map_parse(&parsed,text) && pa_channel_map_equal(&value,&parsed));
    }
    pa_channel_map alsa,wave;assert(pa_channel_map_init_auto(&alsa,6,PA_CHANNEL_MAP_ALSA));
    assert(pa_channel_map_init_auto(&wave,6,PA_CHANNEL_MAP_WAVEEX));
    assert(alsa.map[2]==PA_CHANNEL_POSITION_REAR_LEFT && wave.map[2]==PA_CHANNEL_POSITION_FRONT_CENTER);
    assert(!pa_channel_map_init_auto(&alsa,7,PA_CHANNEL_MAP_ALSA));
    alsa.channels=255;assert(!pa_channel_map_valid(&alsa) && !pa_channel_map_equal(&alsa,&wave));
    pa_cvolume volume;assert(pa_cvolume_set(&volume,2,PA_VOLUME_NORM));
    assert(pa_sw_cvolume_multiply_scalar(&volume,&volume,PA_VOLUME_NORM/2));
    assert(pa_cvolume_avg(&volume)==PA_VOLUME_NORM/2);
    assert(pa_sw_volume_multiply(PA_VOLUME_MAX,PA_VOLUME_MAX)==PA_VOLUME_MAX);
}
int main(void) {
    Display *d = XOpenDisplay(NULL); assert(d);
    int major=2,minor=4,count=-1;
    assert(XIQueryVersion(d,&major,&minor)==Success && major==2 && minor==0);
    XIDeviceInfo *devices=XIQueryDevice(d,XIAllDevices,&count);
    assert(devices && count==4);
    for (int i=0;i<count;i++) {
        assert(devices[i].deviceid==i+2 && devices[i].enabled);
        assert(*devices[i].name && devices[i].num_classes>0);
        assert(devices[i].classes[0]);
    }
    int pointer=0;assert(XIGetClientPointer(d,DefaultRootWindow(d),&pointer) && pointer==2);
    unsigned char bytes[3]={0};XISetMask(bytes,XI_RawMotion);
    XIEventMask mask={XIAllMasterDevices,sizeof bytes,bytes};
    assert(XISelectEvents(d,DefaultRootWindow(d),&mask,1)==Success);
    masks(d,1);
    XErrorHandler old=XSetErrorHandler(error_handler);
    XIBarrierReleasePointer(d,2,1,1);
    assert(XISelectEvents(d,999,&mask,1)==BadValue);
    assert(errors==2);
    image_formats(d);audio_parameters();
    XRRScreenConfiguration *config=XRRGetScreenInfo(d,DefaultRootWindow(d));assert(config);
    XRRScreenSize *sizes=XRRConfigSizes(config,&count);assert(sizes && count==1 && sizes[0].width>0);
    Rotation rotation=0;assert(XRRConfigCurrentConfiguration(config,&rotation)==0 && rotation==RR_Rotate_0);
    assert(XRRSetScreenConfigAndRate(d,config,DefaultRootWindow(d),0,RR_Rotate_0,0,CurrentTime)==0);
    assert(XRRSetScreenConfigAndRate(d,config,DefaultRootWindow(d),1,RR_Rotate_0,0,CurrentTime)!=0);
    XRRScreenResources *resources=XRRGetScreenResourcesCurrent(d,DefaultRootWindow(d));assert(resources && resources->nmode==1);
    assert(resources->modes[0].width==(unsigned)sizes[0].width);
    XRRCrtcGamma *gamma=XRRAllocGamma(3);assert(gamma && gamma->size==3);gamma->blue[2]=123;

    XImage *image=XCreateImage(d,DefaultVisual(d,0),24,ZPixmap,0,NULL,7,5,32,0);
    assert(image && image->bytes_per_line==28 && image->bits_per_pixel==32);
    image->data=calloc(image->height,image->bytes_per_line);assert(image->data);
    assert(XPutPixel(image,2,3,0x123456));
    assert(XGetPixel(image,2,3)==0x123456);
    XImage *sub=XSubImage(image,1,2,3,3);assert(sub);
    assert(XGetPixel(sub,1,1)==0x123456);XDestroyImage(sub);
    Pixmap pixmap=XCreatePixmap(d,DefaultRootWindow(d),7,5,24);assert(pixmap);
    GC gc=XCreateGC(d,pixmap,0,NULL);assert(gc);
    XPutImage(d,pixmap,gc,image,0,0,0,0,7,5);
    XImage *readback=XGetImage(d,pixmap,0,0,7,5,AllPlanes,ZPixmap);assert(readback);
    assert(XGetPixel(readback,2,3)==0x123456);XDestroyImage(readback);
    XFreeGC(d,gc);XFreePixmap(d,pixmap);

    snd_pcm_sw_params_t *sw=NULL;assert(!snd_pcm_sw_params_malloc(&sw));
    assert(!snd_pcm_sw_params_set_start_threshold(NULL,sw,UINT64_C(1)<<40));
    snd_pcm_hw_params_t *hw=NULL;assert(!snd_pcm_hw_params_malloc(&hw));
    assert(!snd_pcm_hw_params_any(NULL,hw));
    snd_pcm_uframes_t frames=UINT64_MAX;
    assert(!snd_pcm_hw_params_get_buffer_size_max(hw,&frames) && frames==65536);
    snd_pcm_t *capture=NULL;
    assert(snd_pcm_open(&capture,"default",SND_PCM_STREAM_CAPTURE,0)<0 && !capture);

    pid_t child=fork();assert(child>=0);
    if (!child) {
        XIBarrierReleasePointer(d,2,1,1);assert(errors==3);
        // Query records, event selections and image payloads are guest storage.
        masks(d,1);assert(!strcmp(devices[0].name,"Virtual core pointer"));
        assert(XGetPixel(image,2,3)==0x123456);
        assert(XRRConfigSizes(config,&count)->width>0 && resources->modes[0].width>0);
        assert(gamma->blue[2]==123);XRRFreeGamma(gamma);
        XRRFreeScreenConfigInfo(config);XRRFreeScreenResources(resources);
        frames=UINT64_MAX;assert(!snd_pcm_hw_params_get_buffer_size_max(hw,&frames) && frames==65536);
        snd_pcm_hw_params_free(hw);snd_pcm_sw_params_free(sw);
        memset(bytes,0,sizeof bytes);assert(!XISelectEvents(d,DefaultRootWindow(d),&mask,1));
        masks(d,0);XIFreeDeviceInfo(devices);XDestroyImage(image);_exit(0);
    }
    int status;assert(waitpid(child,&status,0)==child && status==0);
    masks(d,1);assert(XGetPixel(image,2,3)==0x123456);
    assert(errors==2 && XSetErrorHandler(old)==error_handler);
    assert(gamma->blue[2]==123);XRRFreeGamma(gamma);
    XRRFreeScreenConfigInfo(config);XRRFreeScreenResources(resources);
    snd_pcm_hw_params_free(hw);snd_pcm_sw_params_free(sw);
    XIFreeDeviceInfo(devices);XDestroyImage(image);XCloseDisplay(d);
    puts("MEDIA_XINPUT_IMAGE_PARAMS_OK");return 0;
}
