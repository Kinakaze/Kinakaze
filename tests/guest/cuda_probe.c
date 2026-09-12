/* Freestanding Linux executable: no CUDA SDK or host CUDA runtime dependency. */
typedef unsigned long size_t;
typedef unsigned long long DevicePtr;
typedef void *Handle;
typedef int Result;
extern int printf(const char *, ...), strcmp(const char *, const char *);
extern int fork(void), waitpid(int, int *, int);
extern int execl(const char *, const char *, ...);
extern void exit(int) __attribute__((noreturn));
extern Result cuInit(unsigned), cuDriverGetVersion(int *), cuDeviceGetCount(int *);
extern Result cuDeviceGet(int *, int), cuDeviceGetName(char *, int, int), cuDeviceTotalMem_v2(size_t *, int);
extern Result cuGetErrorName(Result, const char **), cuGetErrorString(Result, const char **);
extern Result cuCtxCreate_v2(Handle *, unsigned, int), cuCtxDestroy_v2(Handle), cuCtxGetCurrent(Handle *);
extern Result cuMemAlloc_v2(DevicePtr *, size_t), cuMemFree_v2(DevicePtr), cuMemHostAlloc(void **, size_t, unsigned), cuMemFreeHost(void *);
extern Result cuMemcpyHtoDAsync_v2(DevicePtr, const void *, size_t, Handle), cuMemcpyDtoHAsync_v2(void *, DevicePtr, size_t, Handle);
extern Result cuMemsetD32Async(DevicePtr, unsigned, size_t, Handle);
extern Result cuStreamCreate(Handle *, unsigned), cuStreamDestroy_v2(Handle), cuStreamSynchronize(Handle);
extern Result cuEventCreate(Handle *, unsigned), cuEventRecord(Handle, Handle), cuEventSynchronize(Handle), cuEventDestroy_v2(Handle), cuEventElapsedTime(float *, Handle, Handle);
extern Result cuModuleLoad(Handle *, const char *), cuModuleUnload(Handle), cuModuleGetFunction(Handle *, Handle, const char *);
extern Result cuLaunchKernel(Handle, unsigned, unsigned, unsigned, unsigned, unsigned, unsigned, unsigned, Handle, void **, void **);
extern Result cuGetProcAddress(const char *, void **, int, unsigned long long);
extern Result cuGetProcAddress_v2(const char *, void **, int, unsigned long long, int *);

static void require(int value, const char *what) {
    if (!value) { printf("CUDA_CHECK_FAILED %s\n", what); exit(91); }
}
static void check(Result result, const char *what) {
    if (result) {
        const char *name="unknown", *description="";
        cuGetErrorName(result,&name); cuGetErrorString(result,&description);
        printf("CUDA_ERROR %s: %d %s %s\n",what,result,name,description);exit(92);
    }
}
static void child_boundary(int initialized) {
    int child=fork();require(child>=0,"fork");
    if (!child) {
        Result result=cuInit(0);
        if(result!=(initialized?801:0))exit(93);
        if(initialized){execl("/probe","/probe","--child-exec",(char *)0);exit(94);}
        exit(0);
    }
    int status=-1;require(waitpid(child,&status,0)==child && status==0,"CUDA fork ownership boundary");
    printf(initialized?"CUDA_FORK_GUARD_OK\n":"CUDA_FORK_BEFORE_INIT_OK\n");
}
static void queries(void) {
    void *pointer=(void *)1;int status=-1;
    check(cuGetProcAddress_v2("cuDeviceGetCount",&pointer,2000,0,&status),"lookup count");
    require(pointer && status==0,"count bridge");
    int count=-1,direct_count=0;check(((Result(*)(int *))pointer)(&count),"indirect count");
    check(cuDeviceGetCount(&direct_count),"direct count");require(count>0 && count==direct_count,"indirect device count");
    pointer=(void *)1;status=-1;
    check(cuGetProcAddress_v2("cuNoSuchEntry",&pointer,12000,0,&status),"unknown lookup");
    require(!pointer && status==1,"unknown lookup clears output");
    require(cuGetProcAddress("cuNoSuchEntry",&pointer,12000,0)==500 && !pointer,"legacy missing status");
    check(cuGetProcAddress_v2("cuGraphLaunch",&pointer,12000,0,&status),"unbridged host lookup");
    require(!pointer && status==1,"never expose unbridged Windows pointer");
    check(cuGetProcAddress_v2("cuMemAlloc",&pointer,2000,0,&status),"old incompatible memory ABI");
    require(!pointer,"do not map 32-bit allocation ABI to size_t ABI");
    check(cuGetProcAddress_v2("cuGetProcAddress",&pointer,12000,0,&status),"self lookup");
    require(pointer && status==0,"versioned query bridge");
    void *nested=0;
    check(((Result(*)(const char *,void **,int,unsigned long long,int *))pointer)("cuDriverGetVersion",&nested,2020,0,&status),"indirect query");
    int version=0;check(((Result(*)(int *))nested)(&version),"indirect driver version");require(version>=12000,"driver version");
    require(cuGetProcAddress_v2("cuInit",&pointer,2000,3,&status)==1,"invalid lookup flags");
    require(cuGetProcAddress_v2("cuInit",&pointer,999999,0,&status)==1,"future driver version");
    printf("CUDA_PROC_ADDRESS_OK\n");
}

static void compute(const char *module_path) {
    int device=-1,count=0,version=0;char name[256]={0};size_t total=0;
    check(cuDeviceGetCount(&count),"device count");require(count>0,"a physical device");
    check(cuDeviceGet(&device,0),"device ordinal");check(cuDeviceGetName(name,sizeof(name),device),"device name");
    check(cuDriverGetVersion(&version),"driver version");check(cuDeviceTotalMem_v2(&total,device),"device memory");
    require(total>0,"physical memory");printf("CUDA_DEVICE %s driver=%d memory=%lu\n",name,version,total);
    Handle context=0;check(cuCtxCreate_v2(&context,0,device),"create context");
    Handle current=0;check(cuCtxGetCurrent(&current),"current context");require(current==context,"context identity");
    enum { N=1025 };size_t bytes=N*sizeof(unsigned);unsigned *host=0;
    check(cuMemHostAlloc((void **)&host,bytes*3,0),"pinned host allocation");
    unsigned *a=host,*b=host+N,*out=host+2*N;
    for(unsigned i=0;i<N;++i){a[i]=3*i+7;b[i]=100000-i;out[i]=0;}
    DevicePtr da=0,db=0,dc=0;void *allocate=0;
    check(cuGetProcAddress("cuMemAlloc",&allocate,3020,0),"allocation lookup");
    check(((Result(*)(DevicePtr *,size_t))allocate)(&da,bytes),"indirect allocation");
    check(cuMemAlloc_v2(&db,bytes),"allocate b");check(cuMemAlloc_v2(&dc,bytes),"allocate output");
    Handle stream=0,start=0,end=0;
    check(cuStreamCreate(&stream,1),"create nonblocking stream");check(cuEventCreate(&start,0),"create event");check(cuEventCreate(&end,0),"create end event");
    check(cuEventRecord(start,stream),"record start");
    check(cuMemcpyHtoDAsync_v2(da,a,bytes,stream),"upload a");check(cuMemcpyHtoDAsync_v2(db,b,bytes,stream),"upload b");
    check(cuMemsetD32Async(dc,0,N,stream),"clear output");
    Handle module=0,kernel=0;
    require(cuModuleLoad(&module,"/missing-cuda-image")==301 && !module,"missing module file");
    require(cuModuleLoad(&module,"/empty-image")==200 && !module,"empty module image");
    check(cuModuleLoad(&module,module_path),"load guest module file");check(cuModuleGetFunction(&kernel,module,"vector_add"),"get kernel");
    unsigned n=N;void *params[]={&da,&db,&dc,&n};
    check(cuLaunchKernel(kernel,(N+127)/128,1,1,128,1,1,0,stream,params,0),"launch direct kernel");
    check(cuMemcpyDtoHAsync_v2(out,dc,bytes,stream),"download direct result");check(cuStreamSynchronize(stream),"direct completion");
    for(unsigned i=0;i<N;++i)require(out[i]==a[i]+b[i],"direct kernel result");
    check(cuMemsetD32Async(dc,0,N,stream),"reset before indirect launch");
    void *launch=0;int status=-1;
    check(cuGetProcAddress_v2("cuLaunchKernel",&launch,7000,2,&status),"per-thread kernel lookup");
    require(launch && status==0,"per-thread bridge");
    check(((Result(*)(Handle,unsigned,unsigned,unsigned,unsigned,unsigned,unsigned,unsigned,Handle,void **,void **))launch)(kernel,(N+127)/128,1,1,128,1,1,0,stream,params,0),"launch indirect kernel");
    check(cuMemcpyDtoHAsync_v2(out,dc,bytes,stream),"download result");check(cuEventRecord(end,stream),"record end");check(cuEventSynchronize(end),"wait event");
    for(unsigned i=0;i<N;++i)require(out[i]==a[i]+b[i],"kernel computed every element");
    float elapsed=-1;check(cuEventElapsedTime(&elapsed,start,end),"event timing");require(elapsed>=0,"valid GPU timing");
    check(cuStreamSynchronize(stream),"stream completion");check(cuModuleUnload(module),"module unload");
    check(cuEventDestroy_v2(start),"destroy start");check(cuEventDestroy_v2(end),"destroy end");check(cuStreamDestroy_v2(stream),"destroy stream");
    check(cuMemFree_v2(da),"free a");check(cuMemFree_v2(db),"free b");check(cuMemFree_v2(dc),"free output");check(cuMemFreeHost(host),"free pinned host memory");
    check(cuCtxDestroy_v2(context),"destroy context");current=(void *)1;check(cuCtxGetCurrent(&current),"context after destroy");require(current==0,"context released");
    printf("CUDA_KERNEL_STREAM_EVENT_CLEANUP_OK n=%d module=%s\n",N,module_path);
}
__attribute__((noreturn)) void probe_start(size_t *initial) {
    int fork_test=initial[0]>1 && !strcmp(((char **)initial)[2],"--fork");
    if(fork_test)child_boundary(0);
    require(cuInit(1)==1,"invalid init flags");check(cuInit(0),"driver initialization");
    const char *module_path=initial[0]>1 && !strcmp(((char **)initial)[2],"--cubin") ? "/cuda-vector.cubin" : "/cuda-vector.ptx";
    queries();compute(module_path);
    if(initial[0]>1 && !strcmp(((char **)initial)[2],"--child-exec"))printf("CUDA_EXEC_AFTER_FORK_OK\n");
    if(fork_test)child_boundary(1);
    printf("CUDA_DRIVER_OK\n");exit(0);
}
__attribute__((naked,noreturn)) void _start(void) {
    __asm__ volatile("mov %rsp,%rdi\n\tand $-16,%rsp\n\tcall probe_start\n\tud2");
}
