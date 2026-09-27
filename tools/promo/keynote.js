import * as T from 'three';
import {RoomEnvironment} from 'three/addons/environments/RoomEnvironment.js';
import {RoundedBoxGeometry} from 'three/addons/geometries/RoundedBoxGeometry.js';

const film=await fetch('/assets/film.json').then(r=>r.json());
await document.fonts.load('500 40px Noto');await document.fonts.load('40px Latin');
const clamp=x=>Math.min(1,Math.max(0,x)),smooth=x=>{x=clamp(x);return x*x*(3-2*x)},ease=x=>1-(1-clamp(x))**4;
const weight=(t,a,b)=>smooth((t-a+.65)/1.15)*(1-smooth((t-b+.15)/.95));
const renderer=new T.WebGLRenderer({antialias:true,preserveDrawingBuffer:true,alpha:false,powerPreference:'high-performance'});
renderer.setSize(1920,1080);renderer.setPixelRatio(1);renderer.setClearColor(0xf3f8fc);
renderer.toneMapping=T.ACESFilmicToneMapping;renderer.toneMappingExposure=.96;
renderer.shadowMap.enabled=true;renderer.shadowMap.type=T.PCFSoftShadowMap;
document.body.prepend(renderer.domElement);
const scene=new T.Scene();scene.background=new T.Color(0xf3f8fc);scene.fog=new T.Fog(0xf3f8fc,28,58);
const camera=new T.PerspectiveCamera(40,16/9,.05,100);
const pmrem=new T.PMREMGenerator(renderer);scene.environment=pmrem.fromScene(new RoomEnvironment(),.055).texture;
scene.environmentIntensity=.85;
scene.add(new T.HemisphereLight(0xdcefff,0xc3c9d0,1.35));
const light=new T.DirectionalLight(0xffffff,3.2);light.position.set(-4,9,7);light.castShadow=true;light.shadow.mapSize.set(2048,2048);light.shadow.camera.left=-15;light.shadow.camera.right=15;light.shadow.camera.top=14;light.shadow.camera.bottom=-12;light.shadow.normalBias=.035;light.shadow.bias=-.00015;scene.add(light);
const fill=new T.DirectionalLight(0xa0d9ff,1.7);fill.position.set(7,2,-5);scene.add(fill);
const floor=new T.Mesh(new T.PlaneGeometry(180,180),new T.MeshStandardMaterial({color:0xf6f9fc,roughness:.27,metalness:.04}));floor.rotation.x=-Math.PI/2;floor.position.y=-3.1;floor.receiveShadow=true;scene.add(floor);
const ice=new T.MeshPhysicalMaterial({color:0x85c6f0,metalness:.12,roughness:.12,transmission:.65,thickness:1.3,ior:1.43,clearcoat:1,clearcoatRoughness:.13});
const porcelain=new T.MeshPhysicalMaterial({color:0xf7fcff,metalness:.12,roughness:.24,clearcoat:1});
const alloy=new T.MeshPhysicalMaterial({color:0x84afcb,metalness:.75,roughness:.23,clearcoat:.6});
const blue=new T.MeshPhysicalMaterial({color:0x408fbd,metalness:.25,roughness:.18,clearcoat:1});
const glow=new T.MeshStandardMaterial({color:0xa1e5ff,emissive:0x54bded,emissiveIntensity:.35,roughness:.2});
function box(w,h,d,mat=porcelain,r=.12){const m=new T.Mesh(new RoundedBoxGeometry(w,h,d,3,r),mat);m.castShadow=true;m.receiveShadow=true;return m}
function sphere(r,mat){const m=new T.Mesh(new T.SphereGeometry(r,40,24),mat);m.castShadow=true;return m}
function group(){const g=new T.Group();scene.add(g);return g}
function setScale(g,v){g.visible=v>.001;g.scale.setScalar(Math.max(.001,v))}
function textTexture(text,size=100,color='#f8fdff'){
 const c=document.createElement('canvas');c.width=1024;c.height=256;const x=c.getContext('2d');x.font=`500 ${size}px Noto`;x.fillStyle=color;x.textAlign='center';x.textBaseline='middle';x.fillText(text,512,128);const tex=new T.CanvasTexture(c);tex.colorSpace=T.SRGBColorSpace;return tex;
}
function badge(text,w=2.3){const m=new T.Mesh(new T.PlaneGeometry(w,w/4),new T.MeshBasicMaterial({map:textTexture(text),transparent:true,depthWrite:false}));return m}
function tube(points,r=.025,mat=alloy){const curve=new T.CatmullRomCurve3(points.map(p=>new T.Vector3(...p)));const mesh=new T.Mesh(new T.TubeGeometry(curve,96,r,8,false),mat);return {mesh,curve}}
function label(name){const d=document.createElement('div');d.className='world-label';document.body.append(d);return d}
function placeLabel(el,position,content,opacity=1){const v=position.clone().project(camera);el.innerHTML=content;el.style.left=`${(v.x*.5+.5)*1920}px`;el.style.top=`${(-v.y*.5+.5)*1080}px`;el.style.opacity=opacity;el.style.display=opacity>.01?'block':'none'}

// A physical, faceted core: transparent outer volume, satin inset and luminous monogram.
const core=group(),shell=box(2.6,2.6,2.6,ice,.32);core.add(shell);
const inner=box(1.93,1.93,1.93,blue,.24);core.add(inner);
const monogram=new T.Group();const vertical=box(.13,1.18,.08,porcelain,.03);vertical.position.x=-.43;monogram.add(vertical);
for(const sign of [-1,1]){const b=box(.14,.88,.08,porcelain,.025);b.rotation.z=-sign*Math.PI/4;b.position.set(.04,sign*.31,0);monogram.add(b)}
monogram.position.z=1.325;core.add(monogram);
const rings=[];
for(let i=0;i<3;i++){const m=new T.Mesh(new T.TorusGeometry(2.05+i*.24,.025+i*.012,10,150,Math.PI*(1.15+i*.22)),i===1?glow:alloy);m.rotation.set(i*.67,.3+i*.7,i*.6);core.add(m);rings.push(m)}
const coreLabel=label('core');

// File leaves physically unfold and extend along growing paths.
const tree=group();const leaves=[],branches=[],packets=[];
const endpoints=[[-6,1.5,-1],[-3.4,3.1,-2],[-1.2,1.5,-3.5],[2.6,2.8,-2.5],[5,1.7,-1],[6,-.4,1.5],[2.8,-1.5,2.4],[-3.7,-1,2.8]];
endpoints.forEach((p,i)=>{
 const leaf=new T.Group();const paper=box(.88,1.22,.09,i%3===0?ice:porcelain,.07);leaf.add(paper);
 for(let j=0;j<4;j++){const line=box(.48-(j%2)*.1,.035,.01,alloy,.01);line.position.set(-.07,.3-j*.18,.055);leaf.add(line)}
 leaf.userData.target=new T.Vector3(...p);tree.add(leaf);leaves.push(leaf);
 const path=tube([[0,-.4,0],[p[0]*.32,-.8,p[2]*.35],[p[0]*.7,p[1]*.6,p[2]],p],.022,i%2?alloy:glow);tree.add(path.mesh);branches.push(path);
 const dot=sphere(.085,glow);tree.add(dot);packets.push(dot);
});

// Three recognizable physical route metaphors, no comparison-card grid.
const routes=group(),routeObjects=[];const routeLabels=[label(),label(),label()];
for(let k=0;k<3;k++){
 const g=new T.Group();g.position.x=(k-1)*4.9;routes.add(g);routeObjects.push(g);
 if(k===0){for(let j=0;j<4;j++){const b=box(2.5,.27,1.8,j===3?ice:porcelain);b.position.y=j*.52-.7;g.add(b)}}
 if(k===1){for(const x of [-1.1,1.1]){const b=box(.8,1.7,.75,ice);b.position.x=x;g.add(b)}const link=tube([[-1,.4,0],[0,1.3,0],[1,.4,0]],.042,alloy);g.add(link.mesh)}
 if(k===2){for(let j=0;j<2;j++){const m=new T.Mesh(new T.TorusGeometry(.65,.17,16,48),j?alloy:porcelain);m.position.set(j*.95-.45,j*.55-.25,0);g.add(m);for(let q=0;q<10;q++){const tooth=box(.24,.28,.3,alloy,.04);const a=q*Math.PI/5;tooth.position.set(m.position.x+Math.cos(a)*.8,m.position.y+Math.sin(a)*.8,0);tooth.rotation.z=a;g.add(tooth)}}}
}

// Agent tool-call path is backed by the recorded CLI/file round trip.
const agent=group();const orbit=sphere(.72,ice);orbit.position.set(-5,.5,0);agent.add(orbit);
const orbitRing=new T.Mesh(new T.TorusGeometry(1.04,.024,8,90),alloy);orbitRing.rotation.x=1.1;orbitRing.position.copy(orbit.position);agent.add(orbitRing);
const file=box(.94,1.28,.11,porcelain);file.position.set(5,.5,0);agent.add(file);
const channel=tube([[-5,.5,0],[-2.4,.5,.4],[0,.3,0],[2.4,.5,.4],[5,.5,0]],.035,glow);agent.add(channel.mesh);
const responses=[];for(let i=0;i<6;i++){const p=box(.14,.14,.35,i%2?glow:alloy,.03);agent.add(p);responses.push(p)}
const agentLeft=label(),agentRight=label();

// Software imagery is a live texture of the real recording, not decorative UI.
const screenGroup=group(),screenCanvas=document.createElement('canvas');screenCanvas.width=1920;screenCanvas.height=1080;
const sx=screenCanvas.getContext('2d'),screenTexture=new T.CanvasTexture(screenCanvas);screenTexture.colorSpace=T.SRGBColorSpace;screenTexture.minFilter=T.LinearFilter;
const bezel=box(16.13,9.13,.1,alloy,.05);bezel.position.z=-.07;screenGroup.add(bezel);
const display=new T.Mesh(new T.PlaneGeometry(16,9,32,18),new T.MeshBasicMaterial({map:screenTexture,toneMapped:false,side:T.DoubleSide}));screenGroup.add(display);
const videos={};for(const n of ['gnome','gnome-settings','minecraft']){const v=document.createElement('video');v.src=`/assets/${n}-clean.mp4`;v.muted=true;v.preload='auto';v.playsInline=true;await new Promise((ok,fail)=>{v.onloadeddata=ok;v.onerror=fail;v.load()});videos[n]=v}
async function seek(v,t){t=Math.floor(Math.min(v.duration-.05,Math.max(0,t))*24)/24;if(Math.abs(v.currentTime-t)>.014){await new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(new Error('Video seek timed out at '+t)),5000);v.onseeked=()=>{requestAnimationFrame(()=>{clearTimeout(timer);resolve()})};v.currentTime=t})}sx.drawImage(v,0,0,1920,1080);screenTexture.needsUpdate=true}
function terminal(name,progress){
 const s=film.terminals[name][Math.min(240,Math.floor(clamp(progress)*240))];sx.fillStyle='#142431';sx.fillRect(0,0,1920,1080);sx.fillStyle='#244053';sx.fillRect(0,0,1920,72);sx.fillStyle='#adc6d8';sx.font='27px Consolas';sx.fillText({vi:'vi / Kinakaze',vim:'Vim 9.0 / Kinakaze',ssh:'Windows OpenSSH  →  Linux sshd',java:'Linux Java / JavaRuntimeProbe'}[name],38,48);
 sx.font='31px Consolas';s.lines.forEach((line,i)=>{sx.fillStyle=/OK|COMPLETE|CONNECTED/.test(line)?'#a6ddff':'#e7eff6';sx.fillText(line,42,122+i*40)});
 if(!s.hidden){sx.fillStyle='#99d2f4';sx.fillRect(42+s.x*18.6,130+s.y*40,17,3)}screenTexture.needsUpdate=true;
}

// A voxel wave grows from the floor and opens into the actual Minecraft menu.
const voxelGroup=group(),voxels=[];
for(let i=0;i<85;i++){const x=(i%17-8)*.69,z=(Math.floor(i/17)-2)*.69;const b=box(.61,.61,.61,i%7===0?ice:(i%3===0?alloy:porcelain),.06);voxelGroup.add(b);b.userData={x,z,index:i};voxels.push(b)}

const $=id=>document.getElementById(id);const headline=$('headline'),eyebrow=$('eyebrow'),sub=$('sub'),proof=$('proof'),minor=$('minor');
const copy={
 seed:['A NEW EXECUTION PATH','你的 Agent。<br><strong>Linux 工具。</strong>','从一次调用，开始。'],
 routes:['THREE ROUTES','工具之外，<strong>还有环境。</strong>',''],
 growth:['CLOSER TO YOUR WORKSPACE','让路径，<br><strong>在本机生长。</strong>',''],
 hero:['MEET KINAKAZE','<strong>Kinakaze.</strong>','Linux 程序 × Windows 用户态'],
 agent:['CONNECT THE TOOLCHAIN','接入 <strong>Agent。</strong>','命令行入口 → 工具调用'],
 roundtrip:['ONE FILE. ONE ROUND TRIP.','同一份文件。<br><strong>直接读回。</strong>',''],
 vi:['REAL SOFTWARE / REAL CAPTURE','<strong>vi.</strong>',''],vim:['REAL SOFTWARE / REAL CAPTURE','<strong>Vim.</strong>',''],ssh:['REAL SOFTWARE / REAL CAPTURE','<strong>SSH.</strong>',''],
 portal:['OPEN THE VIEW','不止<strong>命令行。</strong>',''],gnome:['DESKTOP / STARTUP CAPTURE','<strong>GNOME.</strong>',''],java:['LINUX JAVA','<strong>Java.</strong>',''],
 voxels:['WHAT COMES NEXT','下一个，<br><strong>是什么？</strong>',''],minecraft:['LINUX CLIENT','<strong>Minecraft.</strong>',''],
 constellation:['MORE POSSIBILITIES','给 Agent，<br><strong>多一条 Linux 执行路径。</strong>','无需管理员权限 / 无需自定义驱动'],
 end:['OPEN SOURCE / EXPERIMENTAL','<strong>Kinakaze.</strong>','让熟悉的程序，遇见新的可能。']
};
const poses={seed:[[7,4.6,15],[-1,0,0]],routes:[[4,4,18],[0,0,0]],growth:[[5,4,16],[0,0,0]],hero:[[5,2.7,10],[1,.3,0]],agent:[[1,3.5,17],[0,0,0]],roundtrip:[[2,3.1,17],[0,0,0]],vi:[[1.1,1.15,15],[0,.3,0]],vim:[[-.65,.65,14.5],[0,.25,0]],ssh:[[.5,.75,14.2],[0,.3,0]],portal:[[3,1.9,9],[0,.2,0]],gnome:[[.3,.5,14.5],[0,.25,0]],java:[[-.4,.6,14.5],[0,.3,0]],voxels:[[6,6,13],[1,-.2,0]],minecraft:[[.35,.3,14.35],[0,.15,0]],constellation:[[5,4,16],[0,0,0]],end:[[4,2.6,11],[1,.5,0]]};
window.renderAt=async t=>{
 let i=film.segments.findLastIndex(s=>t>=s.start);i=Math.max(0,i);const s=film.segments[i],kind=s.scene,local=t-s.start;let start=s.start,end=s.start+s.duration;
 if(kind==='minecraft'){start=94.63;end=111.09}
 const progress=clamp((t-start)/(end-start));const beat=Math.exp(-8*((t%(60/175))/(60/175))**2);
 const entry=ease(local/.9);const sw=kind==='hero'||kind==='end'?1.08:1;
 const p=poses[kind],previous=poses[film.segments[Math.max(0,i-1)].scene];const transition=smooth(local/1.1);
 const cam=new T.Vector3(...previous[0]).lerp(new T.Vector3(...p[0]),transition),look=new T.Vector3(...previous[1]).lerp(new T.Vector3(...p[1]),transition);
 cam.x+=Math.sin(t*.21)*.22;cam.y+=Math.sin(t*.17)*.10;cam.z-=progress*.25;camera.position.copy(cam);camera.lookAt(look);
 setScale(core,Math.max(weight(t,0,6.85)*.48,weight(t,16.45,43.89),weight(t,61.71,67.2)*1.5,weight(t,111.09,128)));
 core.position.set(kind==='hero'||kind==='end'?3.1:0,.1+Math.sin(t*.6)*.1,0);core.rotation.set(.14+Math.sin(t*.26)*.08,.35+.30*Math.sin(t*.23),.06);
 if(kind==='agent'||kind==='roundtrip')core.scale.multiplyScalar(.72);
 shell.rotation.y=Math.sin(t*.3)*.07;inner.scale.setScalar(1+beat*.012);
 rings.forEach((r,j)=>{r.rotation.z=t*(.14+j*.08)+j*.8;r.rotation.y=.5+j*.6+Math.sin(t*.2)*.2});
 const tw=Math.max(weight(t,0,6.85),weight(t,16.45,23.31),weight(t,111.09,119.31));setScale(tree,tw);
 const grow=kind==='seed'?ease((local-.4)/4):kind==='growth'?ease(local/3):ease(local/2);
 leaves.forEach((leaf,j)=>{const v=ease((grow*1.8-j*.075));leaf.scale.setScalar(Math.max(.001,v));leaf.position.copy(leaf.userData.target).multiplyScalar(.3+.7*v);leaf.position.y+=Math.sin(t*.8+j)*.12;leaf.rotation.set(1.3*(1-v)+.12*Math.sin(t+j),.25*Math.sin(t*.3+j),Math.sin(t*.35+j)*.12);const path=branches[j];path.mesh.geometry.setDrawRange(0,Math.floor(path.mesh.geometry.index.count*v/48)*48);packets[j].position.copy(path.curve.getPoint((t*.25+j*.12)%1));packets[j].scale.setScalar(v*(.7+beat*.3))});
 tree.rotation.y=Math.sin(t*.13)*.12;
 const rw=weight(t,6.85,16.45);setScale(routes,rw);routeObjects.forEach((g,j)=>{g.position.y=.25*Math.sin(t*.8+j);g.rotation.y=Math.sin(t*.2+j)*.2;g.scale.setScalar(.85+.15*ease((local-j*.22)/1.1))});
 const aw=weight(t,28.8,43.89);setScale(agent,aw);orbit.rotation.y=t*.8;orbitRing.rotation.z=t*.6;file.rotation.y=Math.sin(t)*.12;
 responses.forEach((m,j)=>{let q=(t*.31+j/6)%1;if(kind==='roundtrip'&&local>3)q=1-q;m.position.copy(channel.curve.getPoint(q));m.rotation.z=t*.6;m.scale.setScalar(.8+beat*.35)});
 const screenKind=['vi','vim','ssh','gnome','java','minecraft'].includes(kind);const scw=Math.max(weight(t,43.89,61.71),weight(t,67.2,89.14),weight(t,94.63,111.09));setScale(screenGroup,scw*.91);
 floor.position.y=-3.1-3*scw;
 screenGroup.position.set(0,.03,-2*(1-scw));screenGroup.rotation.set(.02*Math.sin(t*.21),.025*Math.sin(t*.27)+(1-scw)*.85,.002*Math.sin(t*.4));
 const vertices=display.geometry.attributes.position;for(let v=0;v<vertices.count;v++)vertices.setZ(v,(1-scw)*2.4*Math.sin((vertices.getX(v)+8)*Math.PI/16));vertices.needsUpdate=true;bezel.visible=scw>.98;
 if(screenKind){if(['vi','vim','ssh','java'].includes(kind))terminal(kind,progress);else if(kind==='gnome'){sx.drawImage(videos.gnome,0,0,1920,1080);if(local>5.4){sx.globalAlpha=smooth((local-5.4)/.9);sx.drawImage(videos['gnome-settings'],0,0,1920,1080);sx.globalAlpha=1}screenTexture.needsUpdate=true}else await seek(videos.minecraft,.4+t-start)}
 setScale(voxelGroup,weight(t,89.14,94.63));voxels.forEach((v,j)=>{const a=ease((local-j*.013)/1.4);v.position.set(v.userData.x,-2.4+a*(1+Math.sin(v.userData.x*.8+t*1.1)*.65),v.userData.z);v.scale.y=.01+a*(1.7+.8*Math.sin(v.userData.x*.6+t));v.rotation.y=.1*Math.sin(t*.7+j)});voxelGroup.rotation.y=t*.1;
 // Two-dimensional typography remains outside the software image and moves with the camera beat.
 const c=copy[kind];eyebrow.textContent=c[0];headline.innerHTML=c[1];sub.textContent=c[2];headline.style.fontSize=kind==='hero'||kind==='end'?'122px':kind==='constellation'?'64px':'76px';
 headline.style.transform=`translate3d(${(1-entry)*35}px,${(1-entry)*18}px,0)`;headline.style.opacity=entry;eyebrow.style.opacity=entry;sub.style.opacity=entry;
 if(screenKind){headline.style.top='42px';headline.style.left='340px';headline.style.fontSize='32px';eyebrow.style.opacity=0;sub.style.opacity=0;proof.textContent={vi:'真实终端回放',vim:'真实终端回放',ssh:'Windows 客户端 → Linux sshd',gnome:'启动实录',java:'JIT / 线程 / 文件 / 子进程',minecraft:'主菜单与按钮交互实录'}[kind];}
 else {headline.style.top='188px';headline.style.left='87px';proof.textContent=kind==='agent'||kind==='roundtrip'?'CLI 工具调用示例':kind==='routes'?'不同路线，适合不同工作流':'';}
 if(kind==='routes'){headline.style.fontSize='60px';headline.style.top='152px';}
 if(kind==='growth')headline.style.opacity=entry*(1-.7*smooth((local-4)/2));
 minor.textContent=kind==='roundtrip'?(local<3?'run_linux( sort tasks.txt )':'stdout  →  agent · kinakaze · linux · windows\nexit_code  →  0\nWindows readback  →  MATCH'):kind==='agent'?'tool call → Linux ELF → local result':kind==='minecraft'&&t>104.23?'世界内游玩仍在推进':'';
 minor.style.opacity=entry;minor.style.fontSize=kind==='minecraft'?'22px':'25px';
 $('closing').textContent=kind==='end'?'github.com/Kinakaze/Kinakaze':'';
 $('credit').innerHTML=kind==='end'?'MUSIC · SUMMER TRIANGLE / しゃろう<br>VITS · AI 合成女声<br>实验性项目 · 持续开发':'';
 const ci=film.cues.findLastIndex(c=>t>=c.start);$('caption').style.opacity=ci>=0&&t<=film.cues[ci].end?1:0;$('caption').firstElementChild.textContent=ci>=0?film.cues[ci].text:'';
 renderer.render(scene,camera);
 routeLabels.forEach((el,j)=>placeLabel(el,new T.Vector3((j-1)*4.9,-1.75,0),['WSL<br><span>完整环境 / 配置与维护</span>','远程 SSH<br><span>远端执行 / 文件同步</span>','MSYS2<br><span>Windows 移植工具路线</span>'][j],rw));
 placeLabel(agentLeft,new T.Vector3(-5,-1.1,0),'Agent<br><span>TOOL CALL</span>',aw);placeLabel(agentRight,new T.Vector3(5,-1.1,0),'本地文件<br><span>PREPARED GUEST ROOT</span>',aw);
 coreLabel.style.display='none';
 $('flash').style.opacity=t>127.1?smooth((t-127.1)/.8):0;
 return {scene:kind,triangles:renderer.info.render.triangles};
};
window.filmReady=true;
window.playPreview=()=>{let started=performance.now();const frame=async()=>{await window.renderAt(((performance.now()-started)/1000)%127.9);requestAnimationFrame(frame)};frame()};
await window.renderAt(0);
