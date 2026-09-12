#define _GNU_SOURCE
#include <errno.h>
#include <iconv.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>
#define CHECK(x) do { if (!(x)) { fprintf(stderr,"iconv line %d: %s errno=%d\n",__LINE__,#x,errno); exit(1); } } while (0)
static const unsigned char utf8[]={0x41,0xc3,0xa9,0xe4,0xb8,0xad,0xf0,0x9f,0x98,0x80,0};
static const unsigned char utf16le[]={0x41,0,0xe9,0,0x2d,0x4e,0x3d,0xd8,0,0xde,0,0};
static const unsigned char utf16be[]={0,0x41,0,0xe9,0x4e,0x2d,0xd8,0x3d,0xde,0,0,0};
static const unsigned char utf32le[]={0x41,0,0,0,0xe9,0,0,0,0x2d,0x4e,0,0,0,0xf6,1,0,0,0,0,0};
static const unsigned char utf32be[]={0,0,0,0x41,0,0,0,0xe9,0,0,0x4e,0x2d,0,1,0xf6,0,0,0,0,0};
static void convert(iconv_t cd,const void *source,size_t n,const void *expected,size_t size) {
    char output[128]; memset(output,0xaa,sizeof output);
    char *in=(char*)source,*out=output; size_t left=n, room=sizeof output;
    CHECK(iconv(cd,&in,&left,&out,&room)==0); CHECK(!left && in==(char*)source+n);
    CHECK(out==output+size && room==sizeof output-size); CHECK(!memcmp(output,expected,size));
}
static void roundtrip(const char *encoding,const void *data,size_t size) {
    iconv_t cd=iconv_open(encoding,"UTF-8"); CHECK(cd!=(iconv_t)-1);
    convert(cd,utf8,sizeof utf8,data,size); CHECK(!iconv_close(cd));
    cd=iconv_open("UTF-8",encoding); CHECK(cd!=(iconv_t)-1);
    convert(cd,data,size,utf8,sizeof utf8); CHECK(!iconv_close(cd));
}
static void invalid(const char *from,const unsigned char *data,size_t n,int expected) {
    iconv_t cd=iconv_open("UTF-8",from); CHECK(cd!=(iconv_t)-1);
    char *in=(char*)data,output[32],*out=output; size_t left=n,room=sizeof output; errno=0;
    CHECK(iconv(cd,&in,&left,&out,&room)==(size_t)-1 && errno==expected);
    CHECK(in==(char*)data && left==n && out==output && room==sizeof output); CHECK(!iconv_close(cd));
}
int main(void) {
    roundtrip("UTF-16LE",utf16le,sizeof utf16le); roundtrip("UTF-16BE",utf16be,sizeof utf16be);
    roundtrip("UTF-32LE",utf32le,sizeof utf32le); roundtrip("UTF-32BE",utf32be,sizeof utf32be);
    roundtrip("WCHAR_T",utf32le,sizeof utf32le);
    iconv_t cd=iconv_open("WCHAR_T","UTF-8"); CHECK(cd!=(iconv_t)-1);
    char *in=(char*)utf8,output[64],*out=output; size_t left=sizeof utf8,room=4;
    errno=0; CHECK(iconv(cd,&in,&left,&out,&room)==(size_t)-1 && errno==E2BIG);
    CHECK(in==(char*)utf8+1 && left==sizeof utf8-1 && out==output+4 && room==0);
    room=sizeof output-4; CHECK(!iconv(cd,&in,&left,&out,&room)); CHECK(!memcmp(output,utf32le,sizeof utf32le));
    CHECK(!iconv(cd,0,0,0,0));
    pid_t pid=fork(); CHECK(pid>=0);
    if(!pid) { convert(cd,utf8,sizeof utf8,utf32le,sizeof utf32le); CHECK(!iconv_close(cd)); _exit(0); }
    int status; CHECK(waitpid(pid,&status,0)==pid && WIFEXITED(status) && !WEXITSTATUS(status));
    convert(cd,utf8,sizeof utf8,utf32le,sizeof utf32le); CHECK(!iconv_close(cd));
    const unsigned char partial[]={0xe2,0x82}, bad[]={0xed,0xa0,0x80}, overlong[]={0xc0,0x80};
    invalid("UTF-8",partial,sizeof partial,EINVAL); invalid("UTF-8",bad,sizeof bad,EILSEQ); invalid("UTF-8",overlong,sizeof overlong,EILSEQ);
    const unsigned char high[]={0,0xd8}, low[]={0,0xdc}, invalid32[]={0,0,0x11,0};
    invalid("UTF-16LE",high,sizeof high,EINVAL); invalid("UTF-16LE",low,sizeof low,EILSEQ); invalid("UTF-32LE",invalid32,sizeof invalid32,EILSEQ);
    const unsigned char latin[]={0x41,0xe9,0}; const unsigned char latin_utf8[]={0x41,0xc3,0xa9,0};
    cd=iconv_open("UTF-8","ISO-8859-1"); CHECK(cd!=(iconv_t)-1); convert(cd,latin,sizeof latin,latin_utf8,sizeof latin_utf8); CHECK(!iconv_close(cd));
    cd=iconv_open("LATIN1","UTF-8"); CHECK(cd!=(iconv_t)-1); convert(cd,latin_utf8,sizeof latin_utf8,latin,sizeof latin); CHECK(!iconv_close(cd));
    invalid("ASCII",latin+1,1,EILSEQ);
    errno=0; CHECK(iconv_open("unsupported-encoding","UTF-8")== (iconv_t)-1 && errno==EINVAL);
    errno=0; CHECK(iconv_open("ASCII//TRANSLIT","UTF-8")== (iconv_t)-1 && errno==EINVAL);
    CHECK(iconv_close((iconv_t)-1)==-1 && errno==EBADF);
    puts("ICONV_UNICODE_LATIN1_BOUNDS_ERRORS_FORK_OK"); return 0;
}
