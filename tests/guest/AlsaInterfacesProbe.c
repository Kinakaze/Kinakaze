#define _GNU_SOURCE
#include <alsa/asoundlib.h>
#include <assert.h>
#include <errno.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

static void formats(void) {
    assert(snd_pcm_format_width(SND_PCM_FORMAT_S24_LE)==24);
    assert(snd_pcm_format_physical_width(SND_PCM_FORMAT_S24_LE)==32);
    assert(snd_pcm_format_physical_width(SND_PCM_FORMAT_S24_3LE)==24);
    assert(snd_pcm_format_width(SND_PCM_FORMAT_S20_3LE)==20);
    assert(snd_pcm_format_width(SND_PCM_FORMAT_S18_3LE)==18);
    assert(snd_pcm_build_linear_format(24,32,0,1)==SND_PCM_FORMAT_S24_BE);
    assert(snd_pcm_build_linear_format(24,24,0,0)==SND_PCM_FORMAT_S24_3LE);
    snd_pcm_hw_params_t *hw; snd_pcm_format_mask_t *mask;
    assert(!snd_pcm_hw_params_malloc(&hw) && !snd_pcm_format_mask_malloc(&mask));
    assert(!snd_pcm_hw_params_any(NULL,hw));
    snd_pcm_hw_params_get_format_mask(hw,mask);
    assert(snd_pcm_format_mask_test(mask,SND_PCM_FORMAT_S16_LE));
    assert(!snd_pcm_format_mask_test(mask,SND_PCM_FORMAT_S16_BE));
    unsigned n=0;assert(!snd_pcm_hw_params_get_channels_min(hw,&n) && n==1);
    assert(!snd_pcm_hw_params_get_channels_max(hw,&n) && n==8);
    assert(!snd_pcm_hw_params_set_channels(NULL,hw,2));
    assert(!snd_pcm_hw_params_get_channels_min(hw,&n) && n==2);
    assert(!snd_pcm_hw_params_get_channels_max(hw,&n) && n==2);
    assert(snd_pcm_hw_params_set_channels(NULL,hw,0)<0);
    assert(!snd_pcm_hw_params_set_format(NULL,hw,SND_PCM_FORMAT_S24_3LE));
    snd_pcm_hw_params_get_format_mask(hw,mask);
    assert(snd_pcm_format_mask_test(mask,SND_PCM_FORMAT_S24_3LE));
    assert(!snd_pcm_format_mask_test(mask,SND_PCM_FORMAT_S16_LE));
    pid_t p=fork();assert(p>=0);
    if(!p) { assert(snd_pcm_format_mask_test(mask,SND_PCM_FORMAT_S24_3LE));snd_pcm_format_mask_free(mask);snd_pcm_hw_params_free(hw);_exit(0); }
    int status;assert(waitpid(p,&status,0)==p && !status);
    snd_pcm_format_mask_free(mask);snd_pcm_hw_params_free(hw);
}
static void midi(void) {
    snd_midi_event_t *encoder;assert(!snd_midi_event_new(8,&encoder));
    snd_seq_event_t event;memset(&event,0,sizeof event);event.tag=37;
    assert(snd_midi_event_encode_byte(encoder,0x92,&event)==0);
    assert(snd_midi_event_encode_byte(encoder,60,&event)==0);
    assert(snd_midi_event_encode_byte(encoder,0xf8,&event)==1 && event.type==SND_SEQ_EVENT_CLOCK);
    assert(snd_midi_event_encode_byte(encoder,75,&event)==1);
    assert(event.type==SND_SEQ_EVENT_NOTEON && event.data.note.channel==2 && event.data.note.note==60 && event.data.note.velocity==75 && event.tag==37);
    assert(!snd_midi_event_encode_byte(encoder,61,&event));
    assert(snd_midi_event_encode_byte(encoder,0,&event)==1 && event.data.note.note==61);
    unsigned char bytes[]={0xf0,0x7e,0x7f,0x09,0x01,0xf7};
    for(unsigned i=0;i<sizeof bytes;i++) assert(snd_midi_event_encode_byte(encoder,bytes[i],&event)==(i==sizeof bytes-1));
    assert(event.type==SND_SEQ_EVENT_SYSEX && event.data.ext.len==sizeof bytes && !memcmp(event.data.ext.ptr,bytes,sizeof bytes));
    snd_midi_event_free(encoder);
}
static void playback(void) {
    snd_pcm_t *pcm;assert(!snd_pcm_open(&pcm,"default",SND_PCM_STREAM_PLAYBACK,SND_PCM_NONBLOCK));
    snd_pcm_info_t *info;assert(!snd_pcm_info_malloc(&info) && !snd_pcm_info(pcm,info));
    assert(snd_pcm_info_get_card(info)==-1 && !strcmp(snd_pcm_info_get_id(info),"waveout"));snd_pcm_info_free(info);
    snd_output_t *out;char *text=NULL;size_t length=0;FILE *file=open_memstream(&text,&length);assert(file);
    assert(!snd_output_stdio_attach(&out,file,0) && !snd_pcm_dump(pcm,out));
    assert(!snd_output_flush(out) && strstr(text,"Channels: 2") && strstr(text,"Rate: 44100"));
    assert(!snd_output_close(out) && !fclose(file));free(text);
    assert(!snd_pcm_set_params(pcm,SND_PCM_FORMAT_S16_LE,SND_PCM_ACCESS_RW_INTERLEAVED,2,48000,0,20000));
    struct pollfd fd;assert(snd_pcm_poll_descriptors_count(pcm)==1);
    assert(snd_pcm_poll_descriptors(pcm,&fd,1)==1 && poll(&fd,1,1000)==1);
    unsigned short events=0;assert(!snd_pcm_poll_descriptors_revents(pcm,&fd,1,&events) && (events&POLLOUT));
    short silence[960*2]={0};assert(snd_pcm_writei(pcm,silence,960)==960);
    assert(snd_pcm_wait(pcm,1000)==1);
    snd_pcm_status_t *status;assert(!snd_pcm_status_malloc(&status) && !snd_pcm_status(pcm,status));
    assert(snd_pcm_status_get_avail(status)<=960);snd_pcm_status_free(status);
    assert(!snd_pcm_drain(pcm));
    assert(!snd_pcm_prepare(pcm) && snd_pcm_writei(pcm,silence,960)==960);
    int drained=snd_pcm_drain(pcm);assert(drained==0 || drained==-EAGAIN);
    assert(!snd_pcm_nonblock(pcm,0) && !snd_pcm_drain(pcm));
    assert(snd_pcm_state(pcm)==SND_PCM_STATE_SETUP && !snd_pcm_close(pcm));
    snd_mixer_t *mixer;assert(!snd_mixer_open(&mixer,0));
    assert(!snd_mixer_attach(mixer,"default") && !snd_mixer_selem_register(mixer,NULL,NULL) && !snd_mixer_load(mixer));
    snd_mixer_elem_t *element=snd_mixer_first_elem(mixer);
    if(element) {
        long min=-1,max=-1,volume=-1;assert(!strcmp(snd_mixer_selem_get_name(element),"PCM"));
        assert(snd_mixer_selem_has_playback_volume(element) && !snd_mixer_selem_has_capture_volume(element));
        assert(!snd_mixer_selem_get_playback_volume_range(element,&min,&max) && min==0 && max==65535);
        assert(!snd_mixer_selem_get_playback_volume(element,SND_MIXER_SCHN_FRONT_LEFT,&volume) && volume>=min && volume<=max);
        assert(!snd_mixer_elem_next(element));
    }
    assert(!snd_mixer_close(mixer));
}
int main(void) { formats();midi();playback();puts("ALSA_FORMATS_MIDI_POLL_OUTPUT_STATUS_OK");return 0; }
