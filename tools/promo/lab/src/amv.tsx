import React,{useEffect,useMemo,useState} from 'react';
import {AbsoluteFill,Composition,Img,continueRender,delayRender,cancelRender,registerRoot,staticFile,useCurrentFrame} from 'remotion';
import {Audio} from '@remotion/media';
import {ThreeCanvas} from '@remotion/three';
import * as T from 'three';
import settings from '../amv-settings.json';
import './amv.css';
import {EvidenceScene} from './amv-demo';

const FPS=settings.fps,B=.48,DURATION=settings.frames,SCALE=settings.width/settings.designWidth;
const sat=(x:number)=>Math.min(1,Math.max(0,x));
const smooth=(x:number)=>{x=sat(x);return x*x*(3-2*x)};
const out=(x:number)=>1-(1-sat(x))**4;
const mix=(a:number,b:number,t:number)=>a+(b-a)*t;
const pulse=(b:number)=>Math.exp(-(b-Math.floor(b))*13);
type Resources={evidence:any,timeline:any,hero:T.Texture,action:T.Texture};

const vertex=`varying vec2 vUv; uniform float time; void main(){vUv=uv;vec3 p=position;
 float edge=smoothstep(.14,.36,abs(uv.x-.5));
 float hair=smoothstep(.28,.6,uv.y)*(1.-smoothstep(.85,.99,uv.y))*edge;
 float skirt=smoothstep(.18,.3,uv.y)*(1.-smoothstep(.45,.62,uv.y));
 p.x+=sin(time*2.1+uv.y*8.)*7.*hair+sin(time*2.7+uv.y*5.)*3.*skirt;
 p.y+=sin(time*2.+uv.x*7.)*2.*hair+sin(time*2.3+uv.x*4.)*1.4*skirt;
 gl_Position=projectionMatrix*modelViewMatrix*vec4(p,1.);}`;
const fragment=`uniform sampler2D portrait;varying vec2 vUv;void main(){vec4 c=texture2D(portrait,vUv);if(c.a<.012)discard;gl_FragColor=c;#include <colorspace_fragment>}`;

function Actress({tex,t,x=1320,y=565,height=1070,tilt=0,opacity=1}:{tex:T.Texture,t:number,x?:number,y?:number,height?:number,tilt?:number,opacity?:number}){
 const portraitImage=tex.image as HTMLImageElement;const ratio=portraitImage.width/portraitImage.height;
 const uniforms=useMemo(()=>({portrait:{value:tex},time:{value:t}}),[tex]);uniforms.time.value=t;
 return <AbsoluteFill style={{opacity,pointerEvents:'none'}}><ThreeCanvas width={1920} height={1080} dpr={SCALE} gl={{alpha:true,antialias:true}} onCreated={({gl})=>{const s=gl.getDrawingBufferSize(new T.Vector2());console.info(`AMV_DRAWING_BUFFER ${s.x}x${s.y}`)}} camera={{position:[0,0,1000],fov:56.738,near:1,far:2000}}>
  <mesh position={[x-960,540-y+Math.sin(t*2.1)*2.1,0]} rotation={[0,0,tilt+Math.sin(t*1.4)*.003]}>
   <planeGeometry args={[height*ratio,height,36,52]}/><shaderMaterial vertexShader={vertex} fragmentShader={fragment.replace('#include','\n#include').replace('>}','>\n}')} uniforms={uniforms} transparent depthWrite={false}/>
  </mesh>
 </ThreeCanvas></AbsoluteFill>;
}

function Air({t,dark=false}:{t:number,dark?:boolean}){
 return <AbsoluteFill style={{background:dark?'#071929':'linear-gradient(115deg,#fdfcff 10%,#e8f6ff 60%,#c7e4ff 100%)'}}>
  <svg width={1920} height={1080} style={{position:'absolute',opacity:dark?.12:.48}}><defs><linearGradient id="wind" x1="0" x2="1"><stop stopColor="#c2e3ff"/><stop offset="1" stopColor="#fff"/></linearGradient></defs>
   {[0,1,2].map(i=><path key={i} d={`M -100 ${860+i*60} C 420 ${920+i*30}, 920 ${350+Math.sin(t*.35+i)*30}, 2060 ${330+i*150}`} fill="none" stroke="url(#wind)" strokeWidth={24+i*18} transform={`translate(${Math.sin(t*.4+i)*25},${Math.cos(t*.5+i)*16})`}/>)}
  </svg>
  {Array.from({length:18},(_,i)=>{const x=(i*311+t*(8+i%3*7))%2040-60,y=(i*199)%1060;return <div key={i} style={{position:'absolute',left:x,top:y,width:i%3?3:6,height:i%3?3:6,borderRadius:'50%',background:dark?'#8bc7ff':'white',opacity:.5+Math.sin(t+i)*.2}}/>})}
 </AbsoluteFill>;
}
function Close({t,local,scale=1}:{t:number,local:number,scale?:number}){
 return <AbsoluteFill style={{overflow:'hidden'}}><Img src={staticFile(`art/${settings.closeArt}`)} style={{width:1920,height:1080,objectFit:'cover',transform:`translate(${mix(-16,0,out(local/1.7))}px,${Math.sin(t*.6)*2}px) scale(${scale+Math.max(0,1-local/.5)*.02+local*.003})`}}/></AbsoluteFill>;
}
function Mark({dark=false}:{dark?:boolean}){return <div className="amv-mark" style={{color:dark?'#e0f0ff':'#1d446c'}}>kinakaze<span> / </span></div>}
function Streak({p,color='#fff'}:{p:number,color?:string}){
 const v=sat(p);return <AbsoluteFill style={{pointerEvents:'none',overflow:'hidden'}}><div style={{position:'absolute',left:-700+v*3400,top:-250,width:540,height:1700,background:color,transform:'rotate(24deg)',opacity:Math.sin(v*Math.PI)}}/></AbsoluteFill>;
}
function Text({children,x=90,y=390,size=78,color='#23486f',p=1,weight=650}:{children:React.ReactNode,x?:number,y?:number,size?:number,color?:string,p?:number,weight?:number}){return <div style={{position:'absolute',left:x,top:y,fontSize:size,color,fontWeight:weight,letterSpacing:'-.035em',lineHeight:1.2,opacity:out(p),transform:`translateY(${(1-out(p))*28}px)`}}>{children}</div>}
function Strap({small,title,dark=false}:{small:string,title:string,dark?:boolean}){
 return <div className="amv-strap" style={{color:dark?'#e7f3ff':'#234b73'}}><div>{small}</div><strong>{title}</strong></div>;
}

function Film({res}:{res:Resources}){
 const frame=useCurrentFrame(),t=frame/FPS,b=t/B;
 const shot=res.timeline.shots.find((s:any)=>frame>=s.from&&frame<s.to)??res.timeline.shots.at(-1),local=(frame-shot.from)/FPS;
 const actor=res.hero,action=res.action;
 let content:React.ReactNode;
 if(shot.id==='eyes')content=<><Close t={t} local={local}/><Text x={100} y={742} size={54} p={local/.45}>Kinakaze.</Text><div className="opening-caption">LINUX APPS ON WINDOWS</div></>;
 else if(shot.id==='hello')content=<><Air t={t}/><div className="outline-type" style={{transform:`translateX(${-local*35}px)`}}>HELLO.</div><Actress tex={actor} t={t} x={1320-mix(50,0,out(local/.4))} y={540} height={1050}/><Text x={130} y={375} size={113} p={local/.36}>你好，<br/><span style={{color:'#659bf0'}}>Windows。</span></Text><Mark/></>;
 else if(shot.id==='invitation')content=<><Air t={t}/><div className="invitation-screen" style={{transform:`translateX(${mix(-120,0,out(local/.6))}px)`,opacity:out(local/.35)}}><div className="invitation-command"><span>$</span> hello, Linux<span className="typing-caret"/></div><div>在 Windows 上运行 Linux 程序</div></div><Actress tex={action} t={t} x={1390} height={1150} y={596} tilt={-.022}/><Mark/><Streak p={local/.42}/></>;
 else if(shot.id==='pickup'){
  const n=Math.floor((b-12)*2),words=['vi','Vim','SSH','sshd','GNOME','Java','CLI','Kinakaze'];
  content=<><Air t={t} dark={n%2===1}/>{n%3===0&&<div style={{position:'absolute',inset:0,opacity:.22}}><Close t={t} local={local}/></div>}<div className="pickup-word" style={{color:n%2?'#e5f5ff':'#326db5',transform:`scale(${1.02+.04*pulse(b*2)}) rotate(${n%2?-1:1}deg)`}}>{words[Math.min(7,n)]}</div><div className="pickup-sub">LINUX TOOLS · ON WINDOWS</div></>;
 }
 else if(shot.id==='launch')content=<><Air t={t}/><div className="launch-word" style={{transform:`translateX(${(1-out(local/.32))*-140}px) scale(${1+.025*pulse(b)})`}}>Kinakaze<span>.</span></div><Actress tex={action} t={t} x={1410+(1-out(local/.4))*250} height={1150} y={582} tilt={-.035}/><Text x={130} y={632} size={45} p={(local-.15)/.35}>在 Windows 上运行 Linux 程序</Text><Text x={137} y={726} size={24} weight={400} p={(local-.5)/.4}>vi / vim / ssh / GNOME / Java</Text><div className="launch-line" style={{width:730*out(local/.65)}}/><Mark/><Streak p={local/.36}/></>;
 else if(['vi','vim','ssh','desktop','cli','java','minecraft'].includes(shot.id))content=<EvidenceScene kind={shot.id} local={local} t={t} res={res}/>;
 else if(shot.id==='reprise'){
  const minecraft=res.timeline.shots.find((s:any)=>s.id==='minecraft');
  // Let the recording continue beneath an 800 ms dissolve. The portrait
  // then holds continuously, without the former alternating montage.
  content=<>{local<.8&&<EvidenceScene kind="minecraft" local={(frame-minecraft.from)/FPS} t={t} res={res} tailFrames={Math.round(.8*FPS)}/>}<AbsoluteFill style={{opacity:smooth(local/.8)}}><Close t={t} local={local}/></AbsoluteFill></>;
 }
 else content=<><Air t={t}/><div className="end-wind" style={{transform:`translateX(${-local*15}px)`}}>KINAKAZE</div><Actress tex={actor} t={t} x={1410+Math.sin(local*.5)*8} height={1080} y={560}/><Text x={120} y={258} size={142} p={local/.5}>Kinakaze<span style={{color:'#71a1ef'}}>.</span></Text><Text x={130} y={460} size={45} p={(local-.25)/.5}>在 Windows 上运行 Linux 程序</Text><Text x={133} y={586} size={36} weight={500} p={(local-.5)/.45}>现已开源</Text><div className="repo" style={{opacity:out((local-.8)/.5)}}>github.com/Kinakaze/Kinakaze <span>↗</span></div><div className="end-credit">原创角色 / AI 绘制 · VITS 合成女声<br/>Music: しゅわしゅわハニーレモン350ml / しゃろう</div><Mark/></>;
 const swept=['desktop','minecraft'].includes(shot.id);
 const reprise=res.timeline.shots.find((s:any)=>s.id==='reprise');
 return <AbsoluteFill className="amv-film">{content}{swept&&<Streak p={local/.22} color="#f2f7ff"/>}{shot.id==='end'&&local<.8&&<AbsoluteFill style={{opacity:1-smooth(local/.8),pointerEvents:'none'}}><Close t={t} local={(frame-reprise.from)/FPS}/></AbsoluteFill>}<div style={{position:'absolute',inset:0,background:'#fff',opacity:frame<7?1-frame/7:0,pointerEvents:'none'}}/><Audio src={staticFile('audio/mix.wav')}/></AbsoluteFill>;
}

function AMV(){
 const [handle]=useState(()=>delayRender('Load AMV art, fonts and execution evidence'));
 const [res,setRes]=useState<Resources|null>(null);
 useEffect(()=>{
  const texture=(name:string)=>new Promise<T.Texture>((resolve,reject)=>{new T.TextureLoader().load(staticFile(`art/${name}.png`),tex=>{tex.colorSpace=T.SRGBColorSpace;resolve(tex)},undefined,reject)});
  const font=(name:string,file:string)=>new FontFace(name,`url(${staticFile(file)})`,name==='Noto'?{weight:'100 900'}:{}).load().then(f=>(document.fonts as any).add(f));
  Promise.all([fetch(staticFile('evidence.json')).then(r=>r.json()),fetch(staticFile('timeline.json')).then(r=>r.json()),texture('hero'),texture('action'),font('Noto','NotoSansSC.ttf'),font('Mono','consola.ttf')]).then(([evidence,timeline,hero,action])=>{setRes({evidence,timeline,hero:hero as T.Texture,action:action as T.Texture});requestAnimationFrame(()=>continueRender(handle));}).catch(cancelRender);
 },[handle]);
 return res?<AbsoluteFill><div style={{position:'absolute',left:0,top:0,width:1920,height:1080,transform:`scale(${SCALE})`,transformOrigin:'0 0'}}><Film res={res}/></div></AbsoluteFill>:null;
}
registerRoot(()=> <Composition id="Kinakaze-AMV" component={AMV} durationInFrames={DURATION} fps={FPS} width={settings.width} height={settings.height}/>);
