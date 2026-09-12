#include <X11/Xlib.h>
#include <assert.h>
#include <stdio.h>
#include <string.h>
int main(void) {
    _Static_assert(sizeof(XFontStruct)==96,"Linux font layout");
    XCharStruct glyphs[]={{-2,7,6,8,2,123},{0},{1,9,8,7,4,0}};
    XFontStruct f={0}; f.min_char_or_byte2='A'; f.max_char_or_byte2='C';
    f.default_char='C'; f.per_char=glyphs; f.ascent=10; f.descent=5; f.direction=1;
    f.min_bounds=glyphs[0]; f.max_bounds=glyphs[2];
    int dir,ascent,descent; XCharStruct r;
    assert(!XTextExtents(&f,"AB?",3,&dir,&ascent,&descent,&r));
    assert(dir==1 && ascent==10 && descent==5 && r.width==22 && r.lbearing==-2 && r.rbearing==23 && r.ascent==8 && r.descent==4 && r.attributes==123);
    assert(XTextWidth(&f,"AB?",3)==22);
    f.default_char='Z'; assert(XTextWidth(&f,"AB?",3)==6);
    f.min_byte1=f.max_byte1=1; f.min_char_or_byte2=1; f.max_char_or_byte2=3; f.default_char=0x103;
    XChar2b wide[]={{1,1},{1,2},{0,0}};
    assert(!XTextExtents16(&f,wide,3,&dir,&ascent,&descent,&r) && r.width==22 && r.rbearing==23);
    assert(XTextWidth16(&f,wide,3)==22 && XTextWidth(&f,"A",1)==8);
    f.max_byte1=f.min_byte1=0; f.min_char_or_byte2=0x101; f.max_char_or_byte2=0x103;
    assert(XTextWidth16(&f,wide,3)==22);
    f.per_char=0; f.min_bounds=glyphs[0]; f.default_char=0x101;
    assert(XTextWidth16(&f,wide,3)==18);
    assert(!XTextExtents(&f,0,0,&dir,&ascent,&descent,&r) && !r.width && !r.lbearing && !r.rbearing && !r.ascent && !r.descent);
    char many[6000]; memset(many,'A',sizeof(many));
    f.min_char_or_byte2='A'; f.max_char_or_byte2='A'; f.default_char='A';
    assert(XTextWidth(&f,many,sizeof(many))==36000);
    puts("XFONT_METRICS_8_16_DEFAULT_BEARINGS_WIDTH_OK"); return 0;
}
