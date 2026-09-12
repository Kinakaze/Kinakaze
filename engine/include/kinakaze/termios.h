#ifndef KINAKAZE_TERMIOS_H
#define KINAKAZE_TERMIOS_H

#include <kinakaze/types.h>

typedef unsigned char cc_t;
typedef unsigned int speed_t;
typedef unsigned int tcflag_t;

#define NCCS 32

/*
 * The Linux x86_64 struct termios: 60 bytes with c_cc at offset 17. Field order
 * is ABI and must stay in step with kinakaze_vfs::pty::Termios.
 */
struct termios {
    tcflag_t c_iflag;
    tcflag_t c_oflag;
    tcflag_t c_cflag;
    tcflag_t c_lflag;
    cc_t c_line;
    cc_t c_cc[NCCS];
    speed_t c_ispeed;
    speed_t c_ospeed;
};

struct winsize {
    unsigned short ws_row;
    unsigned short ws_col;
    unsigned short ws_xpixel;
    unsigned short ws_ypixel;
};

/* c_lflag */
#define ISIG 0x0001
#define ICANON 0x0002
#define ECHO 0x0008
#define ECHOE 0x0010
#define ECHOK 0x0020
#define ECHONL 0x0040
#define NOFLSH 0x0080
#define TOSTOP 0x0100
#define IEXTEN 0x8000

/* c_iflag */
#define BRKINT 0x0002
#define ISTRIP 0x0020
#define INLCR 0x0040
#define IGNCR 0x0080
#define ICRNL 0x0100
#define IXON 0x0400

/* c_oflag */
#define OPOST 0x0001
#define ONLCR 0x0004

/* c_cflag */
#define CS8 0x0030
#define CREAD 0x0080
#define CLOCAL 0x0800
#define B38400 0x000f

/* c_cc indices */
#define VINTR 0
#define VQUIT 1
#define VERASE 2
#define VKILL 3
#define VEOF 4
#define VTIME 5
#define VMIN 6
#define VSTART 8
#define VSTOP 9
#define VSUSP 10

/* tcsetattr actions */
#define TCSANOW 0
#define TCSADRAIN 1
#define TCSAFLUSH 2

/* tcflush queues */
#define TCIFLUSH 0
#define TCOFLUSH 1
#define TCIOFLUSH 2

/* tcflow actions */
#define TCOOFF 0
#define TCOON 1
#define TCIOFF 2
#define TCION 3

/* Line disciplines */
#define N_TTY 0
#define N_SLIP 1
#define N_MOUSE 2
#define N_PPP 3
#define N_STRIP 4
#define N_AX25 5
#define N_X25 6
#define N_6PACK 7
#define N_MASC 8
#define N_R3964 9
#define N_PROFIBUS_FDL 10
#define N_IRDA 11
#define N_SMSBLOCK 12
#define N_HDLC 13
#define N_SYNC_PPP 14
#define N_HCI 15
#define N_GIGASET_M101 16
#define N_SLCAN 17
#define N_GSM0710 21
#define N_TI_WL 22
#define N_TRACESINK 23
#define N_TRACEROUTER 24
#define N_NCI 25
#define N_SPEAKUP 26
#define N_NULL 27

/* ioctl requests reaching the terminal layer */
#define TCGETS 0x5401
#define TCSETS 0x5402
#define TCSETSW 0x5403
#define TCSETSF 0x5404
#define TCGETA 0x5405
#define TCSETA 0x5406
#define TCSETAW 0x5407
#define TCSETAF 0x5408
#define TCSBRK 0x5409
#define TCXONC 0x540a
#define TCFLSH 0x540b
#define TIOCEXCL 0x540c
#define TIOCNXCL 0x540d
#define TIOCSCTTY 0x540e
#define TIOCGPGRP 0x540f
#define TIOCSPGRP 0x5410
#define TIOCOUTQ 0x5411
#define TIOCSTI 0x5412
#define TIOCGWINSZ 0x5413
#define TIOCSWINSZ 0x5414
#define TIOCMGET 0x5415
#define TIOCMBIS 0x5416
#define TIOCMBIC 0x5417
#define TIOCMSET 0x5418
#define FIONREAD 0x541b
#define TIOCINQ 0x541b
#define TIOCLINUX 0x541c
#define TIOCPKT 0x5420
#define FIONBIO 0x5421
#define TIOCNOTTY 0x5422
#define TIOCSETD 0x5423
#define TIOCGETD 0x5424
#define TCSBRKP 0x5425
#define TIOCSBRK 0x5427
#define TIOCCBRK 0x5428
#define TIOCGSID 0x5429
#define TIOCGPTN 0x80045430
#define TIOCSPTLCK 0x40045431
#define TIOCGDEV 0x80045432
#define TIOCSIG 0x40045436
#define TIOCVHANGUP 0x5437
#define TIOCGPTLCK 0x80045439
#define TIOCGEXCL 0x80045440
#define TIOCGPTPEER 0x5441

#ifdef __cplusplus
extern "C" {
#endif

int tcgetattr(int fd, struct termios *out);
int tcsetattr(int fd, int actions, const struct termios *requested);
void cfmakeraw(struct termios *termios);
int tcflush(int fd, int queue);
int tcdrain(int fd);
int tcflow(int fd, int action);
int tcsendbreak(int fd, int duration);
pid_t tcgetsid(int fd);
pid_t tcgetpgrp(int fd);
int tcsetpgrp(int fd, pid_t pgrp);
speed_t cfgetispeed(const struct termios *termios);
speed_t cfgetospeed(const struct termios *termios);
int cfsetispeed(struct termios *termios, speed_t speed);
int cfsetospeed(struct termios *termios, speed_t speed);
int cfsetspeed(struct termios *termios, speed_t speed);
int ioctl(int fd, unsigned long request, ...);

#ifdef __cplusplus
}
#endif

#endif
