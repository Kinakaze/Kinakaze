#define _GNU_SOURCE
#include <assert.h>
#include <errno.h>
#include <stdio.h>
#include <locale.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>
#include <wchar.h>

struct cookie {
    char bytes[16384];
    size_t length, position, write_limit;
    int reads, writes, closes, fail_read, fail_write, fail_close, open_peer;
};
static ssize_t read_cookie(void *opaque,char *buffer,size_t size) {
    struct cookie *c=opaque;c->reads++;
    if(c->fail_read) {errno=EIO;return -1;}
    size_t left=c->position<c->length?c->length-c->position:0;
    if(size>left)size=left;
    memcpy(buffer,c->bytes+c->position,size);c->position+=size;return size;
}
static ssize_t write_cookie(void *opaque,const char *buffer,size_t size) {
    struct cookie *c=opaque;c->writes++;
    if(c->open_peer) {
        c->open_peer=0;FILE *other=tmpfile();assert(other);
        assert(fputs("peer",other)>=0 && !fclose(other));
    }
    if(c->fail_write) {errno=ENOSPC;return 0;}
    if(c->write_limit && size>c->write_limit) {size=c->write_limit;errno=ENOSPC;}
    assert(c->position+size<=sizeof(c->bytes));
    memcpy(c->bytes+c->position,buffer,size);c->position+=size;
    if(c->position>c->length)c->length=c->position;
    return size;
}
static int seek_cookie(void *opaque,off64_t *offset,int whence) {
    struct cookie *c=opaque;
    off64_t base=whence==SEEK_SET?0:whence==SEEK_CUR?(off64_t)c->position:whence==SEEK_END?(off64_t)c->length:-1;
    if(base<0 || *offset < -base || *offset > (off64_t)sizeof(c->bytes)-base) {errno=EINVAL;return -1;}
    c->position=base+*offset;*offset=c->position;return 0;
}
static int close_cookie(void *opaque) {
    struct cookie *c=opaque;c->closes++;
    if(c->fail_close) {errno=EIO;return -1;}
    return 0;
}
static cookie_io_functions_t hooks={read_cookie,write_cookie,seek_cookie,close_cookie};
static FILE *open_cookie(struct cookie *c,const char *mode) {
    FILE *f=fopencookie(c,mode,hooks);assert(f);return f;
}
static void wait_status(pid_t child,int expected) {
    int status;assert(waitpid(child,&status,0)==child);
    assert(WIFEXITED(status) && WEXITSTATUS(status)==expected);
}
static void operations(void) {
    struct cookie c={0};FILE *f=open_cookie(&c,"w+");
    errno=0;assert(fileno(f)==-1 && errno==EBADF);
    assert(fputs("abcdef",f)>=0 && c.writes==0);
    assert(ftell(f)==6 && !fflush(f) && c.writes==1 && c.length==6);
    assert(!fseek(f,0,SEEK_SET));
    assert(fgetc(f)=='a' && c.reads==1 && ftell(f)==1);
    assert(ungetc('X',f)=='X' && ftell(f)==0 && fgetc(f)=='X');
    assert(!fseek(f,1,SEEK_CUR) && fgetc(f)=='c');
    char tail[10];assert(fread(tail,1,sizeof(tail),f)==3 && !memcmp(tail,"def",3));
    assert(feof(f) && !ferror(f));clearerr(f);assert(!feof(f));
    assert(!fclose(f) && c.closes==1);
    c=(struct cookie){.fail_write=1};f=open_cookie(&c,"w");
    assert(!setvbuf(f,0,_IONBF,0));
    assert(fputc('x',f)==EOF && fputs("text",f)==EOF && fprintf(f,"%d",42)==-1);
    assert(ferror(f) && !fclose(f));

    c=(struct cookie){.bytes="start",.length=5};f=open_cookie(&c,"a+");
    assert(!fseek(f,0,SEEK_SET) && fputs("+",f)>=0 && !fflush(f));
    assert(c.length==6 && !memcmp(c.bytes,"start+",6) && !fclose(f));
    c=(struct cookie){0};f=open_cookie(&c,"r");
    assert(fwrite("x",1,1,f)==0 && ferror(f) && c.writes==0);
    assert(!fclose(f));
    c=(struct cookie){.fail_read=1};f=open_cookie(&c,"r");
    errno=0;assert(fgetc(f)==EOF && ferror(f) && !feof(f) && errno==EIO);
    assert(!fclose(f));

    c=(struct cookie){.write_limit=3};f=open_cookie(&c,"w");
    assert(!setvbuf(f,0,_IONBF,0));
    assert(fwrite("abcdef",1,6,f)==3 && ferror(f) && c.writes==1 && c.length==3);
    assert(!fclose(f) && c.closes==1);
    c=(struct cookie){.fail_write=1,.fail_close=1};f=open_cookie(&c,"w");
    assert(fputs("pending",f)>=0);
    errno=0;assert(fclose(f)==EOF && c.closes==1 && errno==ENOSPC);

    c=(struct cookie){0};cookie_io_functions_t no_seek=hooks;no_seek.seek=0;
    f=fopencookie(&c,"r+",no_seek);assert(f);
    errno=0;assert(fseek(f,0,SEEK_SET)==-1 && errno==ESPIPE);assert(!fclose(f));
    errno=0;assert(!fopencookie(&c,"invalid",hooks) && errno==EINVAL);

    c=(struct cookie){.open_peer=1};f=open_cookie(&c,"w");
    assert(fputs("outer",f)>=0 && !fflush(NULL));
    assert(c.length==5 && !fclose(f));

    c=(struct cookie){.write_limit=3};f=open_cookie(&c,"w");
    struct cookie peer={0};FILE *other=open_cookie(&peer,"w");
    assert(fputs("abcdef",f)>=0 && fputs("peer",other)>=0);
    errno=0;assert(fflush(NULL)==EOF && errno==ENOSPC && ferror(f));
    assert(c.length==3 && peer.length==4);
    c.write_limit=0;assert(!fflush(f) && c.length==6 && !memcmp(c.bytes,"abcdef",6));
    assert(!fclose(f) && !fclose(other));
}
static void wide_operations(void) {
    struct cookie c={.bytes="A\xc3\xa9\xe4\xb8\xad\xf0\x9f\x98\x80\n",.length=11};
    FILE *f=open_cookie(&c,"r");assert(!fwide(f,0));
    assert(fgetwc(f)=='A' && fwide(f,-1)>0);
    assert(getwc(f)==0xe9 && ungetwc(0x3a9,f)==0x3a9 && fgetwc(f)==0x3a9);
    wchar_t line[8];assert(fgetws(line,8,f)==line);
    assert(line[0]==0x4e2d && line[1]==0x1f600 && line[2]=='\n' && line[3]==0);
    assert(fgetwc(f)==WEOF && feof(f) && !ferror(f));
    assert(ungetwc(WEOF,f)==WEOF && feof(f));
    assert(ungetwc(0x20ac,f)==0x20ac && !feof(f) && fgetwc(f)==0x20ac);
    assert(!fclose(f));
    c=(struct cookie){0};f=open_cookie(&c,"w+");assert(fwide(f,1)>0);
    const wchar_t text[]={0x4e2d,0x1f600,'\n',0};
    assert(fputws(text,f)>=0 && putwc(0x20ac,f)==0x20ac && fputwc(0,f)==0);
    assert(!fflush(f) && c.length==12);rewind(f);
    assert(fgetwc(f)==0x4e2d && fgetwc(f)==0x1f600 && fgetwc(f)=='\n');
    assert(fgetwc(f)==0x20ac && fgetwc(f)==0 && !fclose(f));
    for(int truncated=0;truncated<2;truncated++) {
        c=(struct cookie){.bytes="\xf0\x9f",.length=2};
        if(!truncated) {c.bytes[0]=(char)0xff;c.length=1;}
        f=open_cookie(&c,"r");errno=0;
        assert(fgetwc(f)==WEOF && ferror(f) && errno==EILSEQ);assert(!fclose(f));
    }
}
static void generations(void) {
    struct cookie in={.bytes="abcdefgh",.length=8},out={0};
    FILE *input=open_cookie(&in,"r"),*output=open_cookie(&out,"w");
    struct cookie wide={.bytes="A\xe4\xb8\xad\xf0\x9f\x98\x80",.length=8};
    FILE *winput=open_cookie(&wide,"r");
    assert(fgetwc(winput)=='A' && ungetwc(0x3a9,winput)==0x3a9);
    FILE *disk=tmpfile();assert(disk);
    assert(fputs("012345",disk)>=0 && !fflush(disk));rewind(disk);
    assert(fgetc(disk)=='0');
    assert(fgetc(input)=='a' && ungetc('Z',input)=='Z');
    assert(fputs("buffered",output)>=0 && out.length==0);
    pid_t child=fork();assert(child>=0);
    if(!child) {
        assert(fwide(winput,0)>0 && fgetwc(winput)==0x3a9 && fgetwc(winput)==0x4e2d);
        assert(fgetc(disk)=='1' && ftell(disk)==2);
        assert(fgetc(input)=='Z' && fgetc(input)=='b' && in.reads==1);
        assert(!fflush(output) && out.length==8 && !memcmp(out.bytes,"buffered",8));
        assert(fputs(" child",output)>=0 && !fclose(output) && out.closes==1);
        pid_t grandchild=fork();assert(grandchild>=0);
        if(!grandchild) {
            assert(fgetwc(winput)==0x1f600 && !fclose(winput));
            assert(fgetc(input)=='c' && ftell(input)==3 && in.reads==1);
            assert(fgetc(disk)=='2' && !fclose(disk));
            assert(!fclose(input) && in.closes==1);_exit(25);
        }
        wait_status(grandchild,25);
        assert(fgetwc(winput)==0x1f600 && !fclose(winput));
        assert(fgetc(input)=='c' && !fclose(input) && in.closes==1);
        assert(!fclose(disk));_exit(23);
    }
    wait_status(child,23);
    assert(fgetwc(winput)==0x3a9 && fgetwc(winput)==0x4e2d && !fclose(winput));
    assert(out.length==0 && out.closes==0);
    assert(fgetc(input)=='Z' && fgetc(input)=='b' && in.reads==1);
    assert(fgetc(disk)=='1' && !fclose(disk));
    assert(!fflush(output) && out.length==8 && !memcmp(out.bytes,"buffered",8));
    assert(!fclose(output) && out.closes==1 && !fclose(input) && in.closes==1);
}
int main(void) {
    assert(setlocale(LC_ALL, "C.UTF-8"));
    operations();wide_operations();puts("COOKIE_CALLBACKS_WIDE_OK");fflush(stdout);
    generations();puts("COOKIE_FORK_STREAMS_OK");return 0;
}
