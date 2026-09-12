#define _GNU_SOURCE
#include <assert.h>
#include <ctype.h>
#include <errno.h>
#include <langinfo.h>
#include <limits.h>
#include <uchar.h>
#include <locale.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>
#include <wchar.h>
#include <wctype.h>

static locale_t ascii_locale,unicode_locale;
static void *thread(void *unused) {
    (void)unused;
    assert(uselocale(NULL)==unicode_locale);
    assert(towupper(0xe9)==0xc9 && iswalpha(0x4e2d));
    assert(uselocale(ascii_locale)==unicode_locale);
    assert(!iswalpha(0x4e2d) && towupper(0xe9)==0xe9);
    return (void *)23;
}
static void conversions(void) {
    char *end;
    assert(strtod_l(" 12.5tail",&end,ascii_locale)==12.5 && !strcmp(end,"tail"));
    assert(strtof_l("-0x1.8p+2!",&end,unicode_locale)==-6.0f && *end=='!');
    char bytes[16]; wchar_t wide[8]; mbstate_t state={0};
    assert(uselocale(ascii_locale)==unicode_locale);
    assert(!strcmp(nl_langinfo(CODESET),"ANSI_X3.4-1968") && MB_CUR_MAX==1);
    errno=0; assert(mbrtowc(wide,"\xc3\xa9",2,&state)==(size_t)-1 && errno==EILSEQ);
    state=(mbstate_t){0};errno=0;
    memset(bytes,0x55,sizeof bytes);
    assert(wcrtomb(bytes,0xe9,&state)==(size_t)-1 && errno==EILSEQ && bytes[0]==0x55);
    assert(mbtowc(wide,"\xc3\xa9",2)==-1 && wctomb(bytes,0xe9)==-1);
    FILE *ascii=tmpfile();assert(ascii && fwide(ascii,1)>0);
    assert(uselocale(unicode_locale)==ascii_locale);
    assert(!strcmp(nl_langinfo(CODESET),"UTF-8") && MB_CUR_MAX>=4);
    assert(fputwc(0xe9,ascii)==WEOF && ferror(ascii));assert(!fclose(ascii));
    const char *input="\x82\xacZ",*saved=input;
    state=(mbstate_t){0};assert(mbrtowc(wide,"\xe2",1,&state)==(size_t)-2);
    mbstate_t before=state;
    assert(mbsrtowcs(NULL,&input,0,&state)==2 && input==saved && !memcmp(&state,&before,sizeof state));
    assert(mbsrtowcs(wide,&input,1,&state)==1 && wide[0]==0x20ac && *input=='Z' && mbsinit(&state));
    assert(mbsrtowcs(wide+1,&input,7,&state)==1 && !input && wide[1]=='Z' && !wide[2]);
    const wchar_t source[]={0xe9,0x1f642,0},*wp=source;
    assert(wcsrtombs(NULL,&wp,0,&state)==6 && wp==source);
    assert(wcsrtombs(bytes,&wp,1,&state)==0 && wp==source);
    assert(wcsrtombs(bytes,&wp,2,&state)==2 && wp==source+1);
    assert(wcsrtombs(bytes+2,&wp,14,&state)==4 && !wp && !memcmp(bytes,"\xc3\xa9\xf0\x9f\x99\x82",7));
    assert(mbtowc(wide,"\xed\xa0\x80",3)==-1 && wctomb(bytes,0xd800)==-1);
    assert(mbstowcs(NULL,"\xc3\xa9",0)==1 && wcstombs(NULL,source,0)==6);
    struct lconv *lc=localeconv();
    assert(!strcmp(lc->decimal_point,".") && !*lc->thousands_sep);
    assert(lc->frac_digits==CHAR_MAX && lc->int_n_sign_posn==CHAR_MAX);
    assert(!strcmp(nl_langinfo(DAY_1),"Sunday") && !strcmp(nl_langinfo(MON_12),"December"));
    // Stream encoding is captured, independent of the current thread locale.
    FILE *utf=tmpfile();assert(utf && fwide(utf,1)>0);
    assert(uselocale(ascii_locale)==unicode_locale);
    assert(fputwc(0x4e2d,utf)==0x4e2d && !fflush(utf));rewind(utf);
    assert(fgetwc(utf)==0x4e2d);
    pid_t child=fork();assert(child>=0);
    if(!child) { rewind(utf);assert(fgetwc(utf)==0x4e2d && !fclose(utf));_exit(0); }
    int status;assert(waitpid(child,&status,0)==child && status==0);assert(!fclose(utf));
    assert(uselocale(unicode_locale)==ascii_locale);
}
static void wait_exit(pid_t pid,int expected) {
    int status;assert(waitpid(pid,&status,0)==pid);
    assert(WIFEXITED(status) && WEXITSTATUS(status)==expected);
}
int main(void) {
    assert(setlocale(LC_ALL,"C") && !strcmp(setlocale(LC_ALL,NULL),"C"));
    assert(uselocale(NULL)==LC_GLOBAL_LOCALE);
    ascii_locale=newlocale(LC_ALL_MASK,"C",NULL);assert(ascii_locale);
    unicode_locale=newlocale(LC_ALL_MASK,"C.UTF-8",NULL);assert(unicode_locale);
    assert(towupper_l(0xe9,ascii_locale)==0xe9 && towupper_l(0xe9,unicode_locale)==0xc9);
    assert(towlower_l(0x130,unicode_locale)=='i');
    assert(towupper_l(0x1f80,unicode_locale)==0x1f88);
    assert(towupper_l(0xdf,unicode_locale)==0xdf);
    assert(towupper_l(WEOF,unicode_locale)==WEOF);
    assert(!iswalpha_l(0x141,ascii_locale) && iswalpha_l(0x141,unicode_locale));
    assert(!iswdigit_l(0x130,ascii_locale) && !iswdigit_l(0x130,unicode_locale));
    assert(!iswupper_l(0x141,ascii_locale) && !iswupper_l(0x161,ascii_locale));
    assert(iswalpha_l(0x4e2d,unicode_locale) && iswprint_l(0x1f600,unicode_locale));
    assert(!iswprint_l(0xd800,unicode_locale) && !iswprint_l(0x110000,unicode_locale));
    assert(iswspace_l(0x3000,unicode_locale) && !iswspace_l(0xa0,unicode_locale));
    assert(!wctype_l("not-a-class",unicode_locale));
    wctype_t alpha=wctype_l("alpha",unicode_locale);
    assert(alpha && iswctype_l(0x4e2d,alpha,unicode_locale) && !iswctype_l('4',alpha,unicode_locale));
    wctrans_t upper=wctrans_l("toupper",unicode_locale);
    assert(upper && towctrans_l(0x1f80,upper,unicode_locale)==0x1f88 && !wctrans_l("invalid",unicode_locale));
    assert(uselocale(unicode_locale)==LC_GLOBAL_LOCALE);
    assert(wcwidth(0)==0 && wcwidth('a')==1 && wcwidth(0x4e2d)==2 && wcwidth(0x301)==0 && wcwidth(0xd800)==-1);
    conversions();
    pthread_t t;void *result;assert(!pthread_create(&t,NULL,thread,NULL));
    assert(!pthread_join(t,&result) && result==(void *)23);
    assert(uselocale(NULL)==unicode_locale && towupper(0xe9)==0xc9);
    locale_t duplicate=duplocale(unicode_locale);assert(duplicate && duplicate!=unicode_locale);
    locale_t modified=newlocale(LC_CTYPE_MASK,"C",duplicate);assert(modified);duplicate=modified;
    assert(!iswalpha_l(0xe9,duplicate));
    assert(!isalpha_l(0xe9,unicode_locale) && isalpha_l('A',unicode_locale));
    errno=0;assert(!newlocale(LC_ALL_MASK,"not-installed",duplicate) && errno==ENOENT);
    assert(!iswalpha_l(0xe9,duplicate));freelocale(duplicate);
    locale_t legacy=newlocale(1<<LC_ALL,"C",NULL);assert(legacy);freelocale(legacy);
    errno=0;assert(!newlocale((1<<LC_ALL)|LC_CTYPE_MASK,"C",NULL) && errno==EINVAL);
    assert(setlocale(LC_CTYPE,"C.UTF-8"));
    char *saved=strdup(setlocale(LC_ALL,NULL));assert(saved && strstr(saved,"LC_CTYPE=C.UTF-8"));
    assert(setlocale(LC_ALL,"C") && setlocale(LC_ALL,saved));free(saved);
    assert(uselocale(LC_GLOBAL_LOCALE)==unicode_locale && towupper(0xe9)==0xc9);
    assert(uselocale(unicode_locale)==LC_GLOBAL_LOCALE);
    pid_t child=fork();assert(child>=0);
    if(!child) {
        assert(uselocale(NULL)==unicode_locale && towupper(0xe9)==0xc9);
        assert(!strcmp(setlocale(LC_CTYPE,NULL),"C.UTF-8"));
        assert(uselocale(ascii_locale)==unicode_locale && !iswalpha(0x4e2d));
        freelocale(unicode_locale);_exit(25);
    }
    wait_exit(child,25);
    assert(uselocale(NULL)==unicode_locale && towupper(0xe9)==0xc9);
    assert(uselocale(LC_GLOBAL_LOCALE)==unicode_locale);
    freelocale(unicode_locale);freelocale(ascii_locale);
    assert(setlocale(LC_ALL,"C"));puts("LOCALE_RUNTIME_OK");return 0;
}
