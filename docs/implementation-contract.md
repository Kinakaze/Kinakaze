# V2 第一条实现链路的共享接口

范围：独立 workspace、原生 DLL/.so 模块、公共管理进程、runtime RPC 和模块状态生命周期。这里的 fork 事务仅复制/共享已登记的模块状态与逻辑进程身份，不声称完成 guest 内存、寄存器和 Linux fork syscall。

一个活动 worker 对应一个 Linux Process。Init/controller 是宿主基础设施，不占客体 PID。有需要的模块借用同一份 RuntimeApiV1；禁止在每个 DLL 内静态链接 runtime 从而复制全局连接。公开 ABI 定义在 crates/abi，跨进程只传 crates/protocol 的值。

## Protocol / manager 对接

- framing: little-endian u32 payload length + UTF-8 serde JSON，最大 65536 bytes；禁止零长/超长 frame，read_exact 处理截断。
- WireRequest { id: u64, request: Request }; WireResponse { id: u64, result: Result<Reply, RpcError> }。
- RuntimeOpenConfig { endpoint: String, token: String, adoption_ticket: Option<String> }。
- Hello { version: u32, token: String, role: ClientRole, adoption_ticket: Option<String> }。ClientRole = Worker | Controller；version = 1。
- Request: Hello(Hello), Identity, RegisterModule { module_id: u32, schema: u32 }, DefineState { module_id: u32, name: String, initial: u64, fork: ForkPolicy }, ReadState { module_id: u32, name: String }, WriteState { module_id: u32, name: String, value: u64 }, PrepareFork { request_key: u64 }, MarkReady, AwaitActivation, CommitFork { transaction: u64 }, AbortFork { transaction: u64 }, Stats, Shutdown。
- ForkPolicy = Copy | Share | Reset。Reset 恢复 DefineState 的 initial。
- ProcessIdentity { epoch: u64, pid: u32, generation: u32, parent_pid: u32 }。
- ForkTicket { transaction: u64, token: String, child: ProcessIdentity }。
- Reply: Hello { epoch: u64, process: Option<ProcessIdentity> }, Identity(ProcessIdentity), Ok, State { object_id: u64, value: u64 }, ForkPrepared(ForkTicket), Stats(Stats)。
- Stats { processes: usize, clients: usize, objects: usize, transactions: usize }。
- RpcError { code: ErrorCode, message: String }; ErrorCode 包括 InvalidRequest, VersionMismatch, Unauthorized, NotFound, Conflict, NotReady, Aborted, LimitExceeded, Internal。
- Manager API: StateManager::new(epoch: u64, token: String); connect(hello: Hello, peer: PeerIdentity) -> Result<(ClientId, Reply), RpcError>; handle(client: ClientId, request: Request) -> Result<Reply, RpcError>; disconnect(client: ClientId); process_exited(peer: PeerIdentity)。PeerIdentity { host_pid: u32, birth: u64 }; ClientId = u64。
- Prepared -> Adopted -> Ready -> Committed/Aborted；adoption token 单次使用，parent 才能 commit/abort，commit 必须 child Ready。重复 prepare(request_key)、commit/abort 有幂等规则；状态 fork 在 prepare 时形成一致快照。共享值更新共享 ObjectId；copy/reset 为新对象。
- Adopted/Ready child 只允许 Identity、RegisterModule（验证已继承 schema）、MarkReady、AwaitActivation；commit 前不能读写/新建状态或继续 fork。
- AwaitActivation 在未提交时返回 NotReady；服务端以 Condvar 等待状态变化，不能固定周期轮询。
- disconnect 撤销连接和未完成事务；实际 Process 引用以 process_exited 的匹配 native PID/birth 回收，不能仅凭 pipe EOF 宣称宿主进程死亡。
- native parent 退出后当前进程域将存活 child 的 parent_pid 设为 0；getpid 可缓存，getppid 必须刷新。完整 PID namespace init/reaper 后续接入。
- 同一连接的 wire id 从正数开始严格递增，重复序号在执行前拒绝；事务重试使用新 wire id 和同一 request_key/transaction。
- Module/state/request-key 数量和名称长度有硬上限；无裸 HANDLE/指针/host Rust 对象的序列化。

## host-win 对接

- PipeListener::bind(endpoint: &str) -> io::Result<Self>; accept(&mut self) -> io::Result<PipeConnection>。
- PipeConnection::connect(endpoint: &str) -> io::Result<Self>; 实现 Read + Write；peer_pid() -> io::Result<u32>（服务端）。
- pipe 为当前用户权限、PIPE_REJECT_REMOTE_CLIENTS，首次 bind 排除已存在 server。关闭 accept 使用 shutdown flag + 自连接唤醒，不能杀别人的服务。
- ProcessHandle::open(pid: u32) -> io::Result<Self>; identity() -> PeerIdentity 或等价 pid/birth getters；wait() -> io::Result<()>。
- Job::new_kill_on_close() -> io::Result<Self>; assign(&self, &ProcessHandle) -> io::Result<()>；Job 不继承。
- Library::open(path: &Path) -> io::Result<Self>; unsafe symbol(&self, name: &CStr) -> io::Result<*mut c_void>。
- random_token() -> io::Result<String>，BCryptGenRandom 至少 128 bits，不能日志打印 token。
- 可按实现调整细节，但须及时通知 root / runtime / bridge 使用者。全部 Windows native 操作集中在此 crate。
- init 通过 --controller-pid 钉住 Controller 的 PID/birth，Worker 不可用会话 token 提升为 Controller；基础设施进程不可注册 Worker。Controller native 退出会结束会话并关闭所属 Job。

## Runtime / modules 对接

- runtime 的 `open_v1`、`close_v1` 和 `RuntimeApiV1.call` 是固定 C ABI。请求与响应采用调用方提供的有界缓冲区；不把 Rust String/Vec 的所有权跨接口传递。
- 模块按需管理自己的内存、状态和句柄；需要公共生命周期时调用 runtime 或 init RPC。无状态模块不登记额外管理表。
- 管理状态键使用从 SONAME 得到的稳定进程域 ID，碰撞拒绝；并非新增外部清单。
- Linux 函数保持 SysV64 ABI。原生 DLL 内保留带前缀的实现名称；公开 Linux 别名不进入宿主导入库，避免 Windows CRT 同名绑定冲突。
- `kinakaze_module_object_v1(name, length)` 按请求返回数据对象尺寸/对齐，输入只在调用期间借用。PE 导出表没有对象尺寸，不能猜测 COPY 重定位长度。
- fork 通过运行时映射/句柄接口和模块状态序列化接续；DLL loader 重装代码，再由 runtime/模块接口重建状态。完整私有 Rust 状态恢复仍是迁移验收项。

## Loader / package 对接

- 使用文件名和 DLL 原生导出表；不读全局、模块私有或嵌入式清单。
- `.so` 可直接是 PE；Linux 第三方 ELF DSO 继续由 ELF loader 处理。读取失败明确报错。
- `ModuleSet::discover(directory)` 读取真实文件并返回进程内的解析结果；结果不持久化。
- `ModuleImage` 只持有原生 DLL 引用，按原生/版本化名称取得真实地址。旧 ELF 映射跳板已移除。
- 打包从 COFF 导入库读取链接器实际选择的名称，原样复制 DLL；不能改写代码、导入表或标准库分配函数。
- `sdk/` 中的 ELF 仅用于 Linux 编译链接，包含标准 SONAME/符号/版本，不携带模块配置。

## Root 集成职责

root 管 apps/init、apps/worker、跨进程测试、workspace/build 脚本、README。worker 提供可重现的 smoke 场景：父 DLL 定义 Copy/Share/Reset 状态，runtime prepare，新的 Windows child worker adopt/ready，父 commit，子验证 fork 值并修改，父验证复制隔离和共享可见，真实退出后检查对象回收。只运行本次启动的服务和进程。
