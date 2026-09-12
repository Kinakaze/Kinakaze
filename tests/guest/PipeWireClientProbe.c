
typedef unsigned long size_t;
typedef unsigned long long u64;
#define NULL ((void*)0)
#define CHECK(x) do { if (!(x)) return __LINE__; } while (0)
struct iface { const char *type; unsigned version; const void *methods; void *data; };
struct loop { struct iface *system,*loop,*control,*utils;const char *name; };
struct hook { struct hook *next,*prev;const void *funcs;void *data;void (*removed)(struct hook*);void *priv; };
static void remove_hook(struct hook *h) { h->prev->next=h->next;h->next->prev=h->prev;if(h->removed)h->removed(h); }
struct item {const char *key,*value;};
struct dict {unsigned flags,count;const struct item *items;};
struct client_info {unsigned id;u64 changed;const struct dict *props;};
struct permission {unsigned id,permissions;};
struct client_events { unsigned version;void (*info)(void*,const struct client_info*);void (*permissions)(void*,unsigned,unsigned,const struct permission*); };
struct client_methods { unsigned version;
 int (*add)(void*,struct hook*,const struct client_events*,void*);
 int (*error)(void*,unsigned,int,const char*);
 int (*update)(void*,const struct dict*);
 int (*get)(void*,unsigned,unsigned);
 int (*set)(void*,unsigned,const struct permission*);
};
struct core_events {unsigned version;void (*info)(void*,const void*);void (*done)(void*,unsigned,int);void (*ping)(void*,unsigned,int);void (*error)(void*,unsigned,int,int,const char*);};
struct core_methods {unsigned version;
 int (*add)(void*,struct hook*,const struct core_events*,void*);
 void *hello;int (*sync)(void*,unsigned,int);void *pong,*error;
 struct iface* (*registry)(void*,unsigned,size_t);void *create,*destroy;
};
struct registry_events {unsigned version;void (*global)(void*,unsigned,unsigned,const char*,unsigned,const struct dict*);void (*remove)(void*,unsigned);};
struct registry_methods {unsigned version;
 int (*add)(void*,struct hook*,const struct registry_events*,void*);
 struct iface* (*bind)(void*,unsigned,const char*,unsigned,size_t);void *destroy;
};
struct control {unsigned version;int (*fd)(void*);void *hook;void (*enter)(void*),(*leave)(void*);int (*iterate)(void*,int),(*check)(void*);};
extern void *pw_main_loop_new(const void*);
extern struct loop *pw_main_loop_get_loop(void*);
extern int pw_main_loop_run(void*),pw_main_loop_quit(void*);
extern void pw_main_loop_destroy(void*);
extern void *pw_context_new(struct loop*,void*,size_t);
extern void pw_context_destroy(void*);
extern struct iface *pw_context_connect(void*,void*,size_t),*pw_core_get_client(struct iface*);
extern int pw_core_disconnect(struct iface*),pw_core_steal_fd(struct iface*);
extern void pw_proxy_destroy(void*);
extern void *pw_proxy_get_user_data(void*);
extern void *pw_properties_new(const char*,...);
extern int strcmp(const char*,const char*);
static int info_count,done_count,second_done,removed_count,global_count,permission_count,error_count;
static unsigned seen_node,seen_permission_id,seen_permission_bits,seen_index,seen_size;
static int seqs[16];static unsigned ids[16];static char application[64];
static struct hook *self_remove;
static void info(void *p,const struct client_info *i) {
 ++info_count;
 for(unsigned n=0;n<i->props->count;++n) if(!strcmp(i->props->items[n].key,"application.name")) {
  const char *v=i->props->items[n].value;unsigned j=0;while(v[j]&&j<63){application[j]=v[j];++j;}application[j]=0;
 }
 if(self_remove){remove_hook(self_remove);self_remove=NULL;}
}
static void permissions(void *p,unsigned index,unsigned count,const struct permission *values) {
 ++permission_count;seen_index=index;seen_size=count;
 if(count){seen_permission_id=values[count-1].id;seen_permission_bits=values[count-1].permissions;}
}
static void done(void *p,unsigned id,int sequence) {
 if(p){++second_done;return;}
 if(done_count<16){ids[done_count]=id;seqs[done_count]=sequence;}++done_count;
}
static void error(void *p,unsigned id,int sequence,int result,const char *message) {if(id==2&&result==-5&&!strcmp(message,"probe"))++error_count;}
static void global(void *p,unsigned id,unsigned perm,const char *type,unsigned version,const struct dict *props) {
 if(!strcmp(type,"PipeWire:Interface:Node")&&perm&0400){seen_node=id;++global_count;}
}
static void removed(void *p,unsigned id) {if(id==seen_node)++removed_count;}
static void quit_done(void *p,unsigned id,int sequence) {pw_main_loop_quit(p);}
static void destroy_done(void *p,unsigned id,int sequence) {pw_core_disconnect(p);}
int probe(void) {
 void *main=pw_main_loop_new(NULL);CHECK(main);struct loop *l=pw_main_loop_get_loop(main);CHECK(l);
 const struct control *control=l->control->methods;void *cd=l->control->data;
 void *ctx=pw_context_new(l,NULL,0);CHECK(ctx);
 void *props=pw_properties_new("application.name","original",NULL);CHECK(props);
 struct iface *core=pw_context_connect(ctx,props,0),*second=pw_context_connect(ctx,NULL,0);CHECK(core&&second&&core!=second);
 const struct core_methods *cm=core->methods,*cm2=second->methods;
 struct iface *client=pw_core_get_client(core);CHECK(client&&client==pw_core_get_client(core)&&client!=pw_core_get_client(second));
 CHECK(client->version==3&&!strcmp(client->type,"PipeWire:Interface:Client"));
 const struct client_methods *cl=client->methods;
 struct hook core_hook={0},second_hook={0},client_hook={0},registry_hook={0};
 const struct core_events ce={0,NULL,done,NULL,error};const struct client_events cle={0,info,permissions};
 CHECK(cm->add(core->data,&core_hook,&ce,NULL)==0&&cm2->add(second->data,&second_hook,&ce,(void*)1)==0);
 CHECK(cl->add(client->data,&client_hook,&cle,NULL)==0);
 struct iface *registry=cm->registry(core->data,3,64);CHECK(registry);
 unsigned char *scratch=pw_proxy_get_user_data(registry);CHECK(scratch);for(unsigned i=0;i<64;++i)CHECK(scratch[i]==0);
 scratch[63]=0xaa;const struct registry_methods *rm=registry->methods;const struct registry_events re={0,global,removed};
 CHECK(rm->add(registry->data,&registry_hook,&re,NULL)==0);
 char text[]="copied";struct item values[]={{"application.name",text}};struct dict changes={0,1,values};
 CHECK(cl->update(client->data,&changes)==1);text[0]='!';
 int first=cm->sync(core->data,7,0),next=cm->sync(core->data,9,first);CHECK(first>=0&&next>first&&cm2->sync(second->data,0,0)>=0);
 control->enter(cd);CHECK(control->iterate(cd,0)>=0);
 CHECK(info_count==1&&!strcmp(application,"copied")&&done_count==2&&second_done==1);
 CHECK(ids[0]==7&&ids[1]==9&&seqs[0]==first&&seqs[1]==next&&global_count==1&&seen_node!=1);
 CHECK(scratch[63]==0xaa);struct iface *node=rm->bind(registry->data,seen_node,"PipeWire:Interface:Node",3,32);CHECK(node);
 unsigned char *node_data=pw_proxy_get_user_data(node);CHECK(node_data&&node_data!=scratch);for(unsigned i=0;i<32;++i)CHECK(node_data[i]==0);
 pw_proxy_destroy(node);
 // Invalid dictionaries do not partially replace strings or corrupt the dict.
 struct item invalid[]={{"application.name","bad"},{NULL,"bad"}};struct dict bad={0,2,invalid};
 CHECK(cl->update(client->data,&bad)==-22);CHECK(cl->get(client->data,0,16)==0&&control->iterate(cd,0)>=0);
 CHECK(info_count==1&&seen_size==1&&seen_permission_id==~0u);
 CHECK(cl->error(client->data,seen_node,-5,"probe")==0&&error_count==1);
 struct permission denied={seen_node,0};CHECK(cl->set(client->data,1,&denied)==0);
 CHECK(cl->get(client->data,1,1)==0&&control->iterate(cd,0)>=0);
 CHECK(removed_count==1&&seen_index==1&&seen_size==1&&seen_permission_id==seen_node&&seen_permission_bits==0);
 CHECK(!rm->bind(registry->data,seen_node,"PipeWire:Interface:Node",3,0));
 struct permission regain={seen_node,0730};CHECK(cl->set(client->data,1,&regain)==0);
 CHECK(cl->get(client->data,1,1)==0&&control->iterate(cd,0)>=0&&seen_permission_bits==0);
 CHECK(cl->get(client->data,99,10)==0&&control->iterate(cd,0)>=0&&seen_index==99&&seen_size==0);
 CHECK(pw_core_steal_fd(core)==-95); // no fabricated protocol fd
 // Removed guest hooks must not fire again. Self-removal is safe in dispatch.
 struct hook once={0};self_remove=&once;CHECK(cl->add(client->data,&once,&cle,NULL)==0);
 CHECK(control->iterate(cd,0)>=0&&info_count==2);
 remove_hook(&client_hook);int count=info_count;
 values[0].value="after-remove";CHECK(cl->update(client->data,&changes)==1&&control->iterate(cd,0)>=0&&info_count==count);
 remove_hook(&core_hook);CHECK(cm->sync(core->data,0,next)>=0&&control->iterate(cd,0)>=0&&done_count==2);
 remove_hook(&registry_hook);pw_proxy_destroy(registry);
 pw_core_disconnect(core);CHECK(second_hook.next&&second_hook.prev);
 CHECK(cm2->sync(second->data,0,0)>=0&&control->iterate(cd,0)>=0&&second_done==2);
 remove_hook(&second_hook);pw_core_disconnect(second);
 // The real Portal pattern: queue a sync then run until its done event quits.
 core=pw_context_connect(ctx,NULL,0);CHECK(core);cm=core->methods;struct hook quit_hook={0};
 const struct core_events quit_events={0,NULL,quit_done,NULL,NULL};CHECK(cm->add(core->data,&quit_hook,&quit_events,main)==0);
 CHECK(cm->sync(core->data,0,0)>=0&&pw_main_loop_run(main)==0);remove_hook(&quit_hook);pw_core_disconnect(core);
 // A callback can disconnect its core even with a second done event pending.
 core=pw_context_connect(ctx,NULL,0);CHECK(core);cm=core->methods;struct hook dying={0};
 const struct core_events dying_events={0,NULL,destroy_done,NULL,NULL};CHECK(cm->add(core->data,&dying,&dying_events,core)==0);
 CHECK(cm->sync(core->data,0,0)>=0&&cm->sync(core->data,0,0)>=0&&control->iterate(cd,0)>=0);
 CHECK(dying.next==&dying&&dying.prev==&dying);
 control->leave(cd);pw_context_destroy(ctx);pw_main_loop_destroy(main);return 0;
}
