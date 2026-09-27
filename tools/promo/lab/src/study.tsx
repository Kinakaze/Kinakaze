import React,{Suspense,useEffect,useLayoutEffect,useMemo,useState} from 'react';
import {AbsoluteFill,Sequence,staticFile,useCurrentFrame,useVideoConfig,delayRender,continueRender,cancelRender,interpolate,Easing} from 'remotion';
import {Audio,Video} from '@remotion/media';
import {ThreeCanvas} from '@remotion/three';
import {useLoader,useThree} from '@react-three/fiber';
import * as T from 'three';
import {GLTFLoader} from 'three/examples/jsm/loaders/GLTFLoader.js';
import {RoomEnvironment} from 'three/examples/jsm/environments/RoomEnvironment.js';
import {RoundedBoxGeometry} from 'three/examples/jsm/geometries/RoundedBoxGeometry.js';

type Kind='bloom'|'prism'|'flow'|'voxel';
const names={bloom:'生长之庭',prism:'冰蓝切面',flow:'穿行之间',voxel:'世界初生'};
const letter={bloom:'A',prism:'B',flow:'C',voxel:'D'};
const clamp=(v:number)=>Math.max(0,Math.min(1,v));
const smooth=(v:number)=>{v=clamp(v);return v*v*(3-2*v)};
const ease=(v:number)=>1-(1-clamp(v))**3;
const w=(t:number,a:number,b:number,fade=.4)=>smooth((t-a)/fade)*smooth((b-t)/fade);
const lerp=(a:number,b:number,p:number)=>a+(b-a)*p;
const beat=60/175;

function Env(){
 const {gl,scene}=useThree();
 useLayoutEffect(()=>{
  gl.toneMapping=T.ACESFilmicToneMapping;gl.toneMappingExposure=.88;
  const context=gl.getContext(),debug=context.getExtension('WEBGL_debug_renderer_info');if(debug)console.info('KINAKAZE_GPU '+context.getParameter(debug.UNMASKED_RENDERER_WEBGL));
  const pm=new T.PMREMGenerator(gl),env=new RoomEnvironment();const rt=pm.fromScene(env,.025);
  scene.environment=rt.texture;scene.environmentIntensity=.78;
  return ()=>{rt.dispose();pm.dispose();env.dispose();scene.environment=null};
 },[gl,scene]);
 return <><hemisphereLight args={['#ebf7ff','#8198b0',1.1]}/><directionalLight position={[-4,10,6]} intensity={3.2} color="#fffafa" castShadow shadow-mapSize={[2048,2048]} shadow-camera-left={-13} shadow-camera-right={13} shadow-camera-top={14} shadow-camera-bottom={-14} shadow-normalBias={.018} shadow-bias={-.0001}/><directionalLight position={[6,6,-8]} intensity={2.4} color="#9ed5ff"/><directionalLight position={[-8,3,-3]} intensity={1.0} color="#ffffff"/><mesh rotation={[-Math.PI/2,0,0]} position={[0,-.46,0]} receiveShadow><planeGeometry args={[200,200]}/><meshStandardMaterial color="#eaf1f7" roughness={.36} metalness={.08}/></mesh></>;
}

function Rig({kind,t}:{kind:Kind,t:number}){
 const {camera}=useThree();
 useLayoutEffect(()=>{
  let p:number[],q:number[],fov=38;
  if(kind==='bloom'){
   const a=smooth(t/4.1),b=smooth((t-7)/3.2),orbit=t*.055;
   p=[lerp(2.1,7.4,a)+Math.sin(orbit)*1.1,lerp(1.65,8.0,a),lerp(5.1,10.0,a)];
   q=[lerp(.3,-1.0,b),lerp(.45,.8,a),0.3];fov=lerp(42,37,a);
  }else if(kind==='prism'){
   const a=smooth(t/3.5),b=smooth((t-4.1)/3.6),c=smooth((t-9)/2.1);
   p=[lerp(3.0,5.8,a),lerp(2.0,4.4,a),lerp(2.2,9.8,a)];
   p[0]+=Math.sin(b*1.3)*1.7;p[2]-=b*1.2;p[0]=lerp(p[0],5.7,c);p[2]=lerp(p[2],12.1,c);
   q=[lerp(.0,-1.35,c),1.35,0];fov=38;
  }else if(kind==='flow'){
   if(t<9.6){
    const a=smooth(t/2.75),b=smooth((t-2.75)/6.85);
    p=[lerp(4.6,.0,a)+Math.sin(t*.75)*.12,lerp(3.5,2.12,a),lerp(10.5,8.0,a)-b*12.0];q=[0,2.05,p[2]-10];fov=lerp(44,56,b);
   }else{
    const a=smooth((t-9.6)/2.3);p=[lerp(8,10.0,a),lerp(5.7,7.1,a),lerp(13,15,a)];q=[-2.9,1.25,0];fov=42;
   }
  }else{
   const a=smooth(t/3.7),b=smooth((t-4.1)/5.5),c=smooth((t-10.6)/2.3);
   const theta=lerp(.08,.95,a)+b*.32;
   p=[Math.sin(theta)*lerp(5.8,10.4,a),lerp(2.7,8.0,a),Math.cos(theta)*lerp(5.8,10.4,a)];
   q=[lerp(.0,-1.6,c),lerp(.45,1.25,a),0];fov=lerp(45,38,a);
  }
  camera.position.set(p[0],p[1],p[2]);camera.lookAt(q[0],q[1],q[2]);(camera as T.PerspectiveCamera).fov=fov;camera.updateProjectionMatrix();
 },[t,kind,camera]);return null;
}

function Asset({kind,t,gltf}:{kind:Kind,t:number,gltf:any}){
 const scene=useMemo(()=>{
  if(!gltf)return null;
  const cloned=gltf.scene.clone(true);
  cloned.traverse((o:any)=>{
   o.userData.baseP=o.position.clone();o.userData.baseR=o.rotation.clone();o.userData.baseS=o.scale.clone();
   if(o.isMesh){o.castShadow=true;o.receiveShadow=true;const mats=Array.isArray(o.material)?o.material:[o.material];o.material=mats.map((m:T.MeshStandardMaterial)=>{const n=m.clone();n.envMapIntensity=1.15;return n});if(!Array.isArray(gltf.scene.getObjectByName(o.name)?.['material']))o.material=o.material[0];}
  });return cloned;
 },[gltf]);
 useLayoutEffect(()=>{
  if(!scene)return;
  scene.traverse((o:any)=>{
   const {baseP:p,baseR:r,baseS:s}=o.userData;if(!p)return;
   o.position.copy(p);o.rotation.copy(r);o.scale.copy(s);
   if(kind==='bloom'){
    if(/^spin_\d+$/.test(o.name))o.rotation.y=r.y+t*(o.name==='spin_09'?-.12:.25);
    if(/^rise_\d+$/.test(o.name)){const idx=Number(o.name.split('_')[1]);const a=ease((t-.5-idx*.20)/2.3);o.scale.y=s.y*Math.max(.002,a);}
    if(/^petal_\d+$/.test(o.name)){const idx=Number(o.name.split('_')[1]);const a=smooth((t-1.1-idx*.035)/2.4);o.rotation.z=r.z+lerp(-1.15,.0,a);o.scale.setScalar(Math.max(.001,a));}
    if(o.name==='brand'){const a=ease((t-2.2)/2.1);o.position.y=p.y-.9*(1-a);o.scale.multiplyScalar(Math.max(.001,a));o.rotation.y=r.y+Math.sin(t*.32)*.14;}
    if(o.name.startsWith('orbital_'))o.rotation.y+=t*.065;
   }
   if(kind==='prism'){
    if(/^wafer_\d+$/.test(o.name)){const idx=Number(o.name.split('_')[1]),a=Math.sin(smooth((t-.5)/6.5)*Math.PI);o.position.x=p.x+(idx-3)*a*.22;o.position.z=p.z-a*.28;o.rotation.z=r.z+a*(idx-3)*.075;}
    if(o.name==='brand')o.rotation.y=r.y+Math.sin(t*.37)*.065;
    if(o.name.startsWith('ribbon_'))o.rotation.y=r.y+Math.sin(t*.35)*.12;
   }
   if(kind==='flow'&&/^gate_\d+$/.test(o.name)){const idx=Number(o.name.split('_')[1]);o.rotation.z=r.z+Math.sin(t*.45+idx*.2)*.09;const a=ease((t+1.2-idx*.14)/1.7);o.scale.multiplyScalar(Math.max(.01,a));}
   if(kind==='voxel'){
    if(/^tile_\d+_\d+$/.test(o.name)){
     const dist=Math.hypot(p.x,p.z),a=ease((t-.12-dist*.46)/1.65);o.scale.y=s.y*Math.max(.004,a);
     o.position.y=p.y+Math.sin(dist*2.2-t*2)*.14*smooth((t-3.2)/1.8);
     const spread=smooth((t-6.8)/1.2)*(1-smooth((t-10.5)/1.0));o.position.x+=p.x*spread*.23;o.position.z+=p.z*spread*.23;
    }
    if(o.name==='brand'){const a=ease((t-2.7)/2.0);o.scale.multiplyScalar(Math.max(.001,a));o.position.y=p.y+(1-a)*2.1;o.rotation.y=r.y+Math.sin(t*.35)*.15;}
    if(o.name.startsWith('horizon_')){o.rotation.y=r.y+t*.06;const a=ease((t-2.4)/2.6);o.scale.multiplyScalar(Math.max(.001,a));}
   }
  });
 },[scene,t,kind]);
 return scene?<primitive object={scene}/>:null;
}

function Paths({kind,t}:{kind:Kind,t:number}){
 const curves=useMemo(()=>Array.from({length:kind==='bloom'?5:3},(_,i)=>{
  if(kind==='bloom'){
   const a=i*Math.PI*2/5+.4;return new T.CatmullRomCurve3([new T.Vector3(0,.25,0),new T.Vector3(Math.cos(a)*.8,1.1,-Math.sin(a)*.8),new T.Vector3(Math.cos(a)*1.8,.9,-Math.sin(a)*1.8),new T.Vector3(Math.cos(a)*2.5,.62,-Math.sin(a)*2.5)]);
  }
  return new T.CatmullRomCurve3(Array.from({length:24},(_,j)=>{const z=8-j*.72;return new T.Vector3(Math.sin(j*.43+i*2.1)*.45,2.1+Math.cos(j*.43+i*2.1)*.45,z)}));
 }),[kind]);
 const geos=useMemo(()=>curves.map(c=>new T.TubeGeometry(c,100,.013,6,false)),[curves]);
 geos.forEach((g,i)=>g.setDrawRange(0,Math.floor(g.index!.count*ease((t-1.1-i*.13)/2.4)/6)*6));
 return <>{curves.map((c,i)=>{
  const p=c.getPoint(((t*(kind==='bloom'?.17:.12)+i*.26)%1));
  return <group key={i}><mesh geometry={geos[i]}><meshStandardMaterial color="#459ded" emissive="#459ded" emissiveIntensity={.35} roughness={.2} metalness={.3}/></mesh><mesh position={p} scale={.065*ease((t-.5)/1)}><sphereGeometry args={[1,18,12]}/><meshStandardMaterial color="#e9fdff" emissive="#59bcff" emissiveIntensity={.9}/></mesh></group>;
 })}</>;
}

function Motes({t,kind}:{t:number,kind:Kind}){
 return <>{Array.from({length:20},(_,i)=>{
 const a=i*2.399+t*.05,r=4.3+(i%4)*.7,y=1.7+Math.sin(i*3.1+t*.4)*1.3;
 return <mesh key={i} position={[Math.cos(a)*r,y,Math.sin(a)*r]} rotation={[t*.2+i,t*.15,i*.3]} scale={.026+(i%3)*.015}><octahedronGeometry/><meshStandardMaterial color={i%3?'#cae3f5':'#438de7'} metalness={.3} roughness={.22}/></mesh>;
 })}</>;
}

function World({kind,t}:{kind:Kind,t:number}){
 const {width,height}=useVideoConfig();
 const [gltf,setGltf]=useState<any>(null);
 const [handle]=useState(()=>delayRender(`Loading Blender geometry: ${kind}`));
 useEffect(()=>{new GLTFLoader().load(staticFile(`models/${kind}.glb`),g=>{setGltf(g);requestAnimationFrame(()=>requestAnimationFrame(()=>continueRender(handle)))},undefined,e=>cancelRender(e));},[kind,handle]);
 if(!gltf)return null;
 return <ThreeCanvas width={width} height={height} shadows={{type:T.PCFShadowMap}} gl={{antialias:true,alpha:false,powerPreference:'high-performance',preserveDrawingBuffer:true}} camera={{fov:38,near:.03,far:150,position:[7,7,10]}}>
  <color attach="background" args={['#eef4f9']}/><fog attach="fog" args={['#eef4f9',35,90]}/><Env/><Rig kind={kind} t={t}/><Asset kind={kind} t={t} gltf={gltf}/>{(kind==='bloom'||kind==='flow')&&<Paths kind={kind} t={t}/>}<Motes kind={kind} t={t}/>
 </ThreeCanvas>;
}

function TypeLine({text,t,start,end,style={},small=false}:{text:string,t:number,start:number,end:number,style?:React.CSSProperties,small?:boolean}){
 return <div style={{position:'absolute',color:'#153651',fontSize:small?31:84,fontWeight:small?400:520,letterSpacing:small?'.04em':'-.055em',lineHeight:1.2,...style}}>{Array.from(text).map((char,i)=><span key={i} style={{display:'inline-block',whiteSpace:'pre',opacity:w(t,start+i*.045,end,.32),transform:`translateY(${(1-ease((t-start-i*.045)/.65))*25}px)`,filter:`blur(${(1-ease((t-start-i*.045)/.5))*8}px)`}}>{char}</span>)}</div>;
}

function Editorial({kind,t}:{kind:Kind,t:number}){
 const frame=useCurrentFrame();
 const labelOpacity=smooth((t-.35)/.7);
 const end=smooth((t-10.1)/.8);
 return <AbsoluteFill style={{pointerEvents:'none'}}>
  <div style={{position:'absolute',left:82,top:49,fontSize:27,letterSpacing:'-.02em',fontWeight:550,color:'#1c3e56',opacity:labelOpacity}}>kinakaze<span style={{color:'#518dbe',fontWeight:300}}> / </span></div>
  <div className="caps" style={{position:'absolute',right:80,top:59,fontSize:17,color:'#5b7a91',opacity:labelOpacity}}>MOTION STUDY {letter[kind]} · {names[kind]}</div>
  {kind==='bloom'&&<>
   <TypeLine text="从一行命令。" t={t} start={.65} end={3.45} style={{left:90,top:168,fontSize:66}}/>
   <TypeLine text="长出更多可能。" t={t} start={4.05} end={8.5} style={{left:90,top:182,fontSize:79}}/>
   <div className="caps" style={{position:'absolute',left:96,top:296,fontSize:22,color:'#417899',opacity:w(t,4.4,8.3)}}>GROW FROM ONE COMMAND</div>
   <div style={{position:'absolute',left:98,top:374,color:'#3373a8',fontSize:31,opacity:w(t,5.0,8.3)}}>vi <span style={{padding:'0 18px',color:'#aac2d2'}}>·</span> vim <span style={{padding:'0 18px',color:'#aac2d2'}}>·</span> ssh</div>
  </>}
  {kind==='prism'&&<>
   <div className="caps" style={{position:'absolute',left:91,top:172,fontSize:25,color:'#275d85',opacity:w(t,.65,3.1)}}>FAMILIAR TOOLS.<br/><span style={{display:'block',marginTop:15}}>A NEW PERSPECTIVE.</span></div>
   <TypeLine text="熟悉的工具。" t={t} start={3.65} end={6.8} style={{left:92,top:188,fontSize:76}}/>
   <TypeLine text="新的可能。" t={t} start={6.7} end={9.7} style={{left:92,top:188,fontSize:82}}/>
  </>}
  {kind==='flow'&&<>
   <TypeLine text="给 Agent，一条新路径。" t={t} start={.6} end={3.25} style={{left:90,top:160,fontSize:67}}/>
   <div style={{position:'absolute',left:0,right:0,top:139,textAlign:'center',opacity:w(t,3.7,6.4),fontSize:35,color:'#216197',letterSpacing:'.04em'}}>Agent <span style={{margin:'0 25px',color:'#89acc7'}}>→</span> CLI <span style={{margin:'0 25px',color:'#89acc7'}}>→</span> Linux</div>
   <div className="mono" style={{position:'absolute',left:93,bottom:170,fontSize:30,lineHeight:1.8,color:'#174463',opacity:w(t,6.4,9.5)}}><span style={{color:'#598cae'}}>stdout</span>　agent · kinakaze · linux · windows<br/><span style={{color:'#598cae'}}>exit code</span>　0</div>
   <div style={{position:'absolute',left:94,bottom:121,color:'#59798f',fontSize:23,opacity:w(t,6.4,9.5)}}>CLI 工具调用示例 · 来自实际执行记录</div>
  </>}
  {kind==='voxel'&&<>
   <TypeLine text="再往前一步。" t={t} start={.55} end={3.6} style={{left:92,top:176,fontSize:73}}/>
   <TypeLine text="会遇见什么？" t={t} start={3.55} end={6.2} style={{left:92,top:182,fontSize:77}}/>
  </>}
  <div style={{position:'absolute',left:91,top:244,opacity:end}}>
   <div className="caps" style={{fontSize:21,color:'#477ea6',marginBottom:28}}>A NEW EXECUTION PATH</div>
   <div style={{fontFamily:'Display',fontSize:125,fontWeight:300,letterSpacing:'-.07em',lineHeight:1,color:'#123e65',transform:`translateY(${(1-end)*25}px)`}}>Kinakaze.</div>
   <div style={{fontSize:31,fontWeight:400,color:'#4b708b',marginTop:34}}>Linux 工具，走进 Windows。</div>
   <div className="caps" style={{fontSize:20,color:'#3b79a7',marginTop:52}}>OPEN SOURCE / MORE POSSIBILITIES</div>
  </div>
  <div style={{position:'absolute',left:92,right:92,bottom:53,display:'flex',justifyContent:'space-between',color:'#6d879a',fontSize:18,opacity:smooth((t-.8)/1)}}><span className="caps">KINAKAZE / {names[kind]}</span><span>VITS 合成女声 · Music / しゃろう</span></div>
  <div style={{position:'absolute',bottom:29,left:92,width:1736*clamp(t/(824/60)),height:1,background:'#8abbd9',opacity:.6}}/>
 </AbsoluteFill>;
}

export function Study({kind}:{kind:Kind}){
 const frame=useCurrentFrame(),t=frame/60;
 const [fontHandle]=useState(()=>delayRender('Load local typography'));
 useEffect(()=>{const noto=new FontFace('Noto',`url(${staticFile('NotoSansSC.ttf')})`,{weight:'100 900'}),latin=new FontFace('Display',`url(${staticFile('calibril.ttf')})`,{weight:'300'});Promise.all([noto.load(),latin.load()]).then(fonts=>{fonts.forEach(f=>(document.fonts as any).add(f));continueRender(fontHandle)});},[fontHandle]);
 const portal=kind==='voxel'?w(t,6.3,10.55,.65):0;
 const wipe=kind==='flow'?Math.max(0,1-Math.abs(t-9.6)/.22):(kind==='prism'?Math.max(0,1-Math.abs(t-3.95)/.24):0);
 return <AbsoluteFill style={{background:'#eef4f9'}}>
  <World kind={kind} t={t}/>
  {kind==='prism'&&<Sequence durationInFrames={237}><Video src={staticFile('blender-macro.mp4')} muted objectFit="cover" style={{position:'absolute',width:'100%',height:'100%'}}/></Sequence>}
  <AbsoluteFill style={{background:'radial-gradient(ellipse at 48% 40%,transparent 43%,rgba(67,103,137,.055) 100%)'}}/>
  {kind==='voxel'&&<><svg width={0} height={0}><defs><clipPath id="pixel-reveal" clipPathUnits="userSpaceOnUse">{Array.from({length:84},(_,i)=>{const x=i%12,y=Math.floor(i/12),p=ease((t-6.3-(x+y)*.016)/.38);return <rect key={i} x={x*160+80*(1-p)} y={y*(1080/7)+(1080/14)*(1-p)} width={160*p+1} height={(1080/7)*p+1}/>})}</clipPath></defs></svg><Sequence from={378} durationInFrames={255}><AbsoluteFill style={{opacity:portal,clipPath:'url(#pixel-reveal)',background:'#0d1821'}}><Video src={staticFile('minecraft.mp4')} trimBefore={420} muted objectFit="contain" style={{width:'100%',height:'100%'}}/><div style={{position:'absolute',left:80,top:64,color:'white',fontSize:28,textShadow:'0 2px 12px #000'}}>Minecraft · Linux 客户端</div><div style={{position:'absolute',right:80,bottom:85,color:'#f2f7fa',fontSize:22,textShadow:'0 2px 12px #000'}}>主菜单实录</div></AbsoluteFill></Sequence></>}
  <AbsoluteFill style={{opacity:1-portal}}><Editorial kind={kind} t={t}/></AbsoluteFill>
  <AbsoluteFill style={{background:'#e9f4ff',opacity:wipe,pointerEvents:'none'}}/>
  <AbsoluteFill style={{background:'#eef4f9',opacity:1-smooth(t/.35),pointerEvents:'none'}}/>
  <Audio src={staticFile(`audio/${kind}.wav`)}/>
 </AbsoluteFill>;
}
