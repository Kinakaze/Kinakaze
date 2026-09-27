import React from 'react';
import {AbsoluteFill,Sequence,staticFile} from 'remotion';
import {Video} from '@remotion/media';
import settings from '../amv-settings.json';
import './amv-demo.css';

const clamp=(v:number)=>Math.min(1,Math.max(0,v));
const ease=(v:number)=>1-(1-clamp(v))**4;
const blend=(a:number,b:number,p:number)=>a+(b-a)*clamp(p);
const typing=(text:string,t:number,start:number,duration:number)=>text.slice(0,Math.floor(text.length*clamp((t-start)/duration)));
type State={lines:string[],x:number,y:number,hidden:boolean};
type Evidence={terminals:Record<string,State[]>,cli:any};

function Command({text}:{text:string}){
 return <>{text.split(/([|>])/).map((part,i)=><span key={i} className={part==='|'||part==='>'?'syntax-operator':undefined}>{part}</span>)}</>;
}
function Rail({steps,index,progress}:{steps:string[],index:number,progress:number}){
 return <div className="demo-steps">{steps.map((step,i)=><React.Fragment key={step}><span className={i===index?'active':i<index?'complete':''}>{step}</span>{i<steps.length-1&&<i><b style={{width:`${clamp(progress-i)*100}%`}}/></i>}</React.Fragment>)}</div>;
}
function Shell({title,label,steps,phase,local,children,cue,continuous=false}:{title:string,label:string,steps:string[],phase:number,local:number,children:React.ReactNode,cue:React.ReactNode,continuous?:boolean}){
 const entrance=continuous?1:ease(local/.35);
 return <AbsoluteFill className="cli-scene">
  <div className="cli-grain-lines"/><div className="cli-brand">kinakaze<span> / </span></div>
  <div className="cli-heading"><small>{label}</small><h2>{title}</h2></div>
  <Rail steps={steps} index={Math.min(steps.length-1,Math.floor(phase))} progress={phase}/>
  <div className="cli-window" style={{opacity:entrance,transform:`translateY(${(1-entrance)*24}px)`}}>
   <div className="cli-chrome"><div className="cli-tab"><span>›_</span> {label==='VI / VIM'?'hello.txt':label==='SHELL / FILES'?'sh · /workspace/promo-cli':label==='SSH / SSHD'?'OpenSSH · Linux session':'OpenJDK · Linux'}</div><span className="cli-chrome-path">{label==='VI / VIM'?'/tmp/hello.txt':'Kinakaze'}</span><span className="cli-chrome-controls">—　□　×</span></div>
   {children}
  </div>
  <div className="cli-cue">{cue}</div><div className="cli-source">真实命令与输出 · 节奏剪辑</div>
 </AbsoluteFill>;
}
function TerminalState({state,kind}:{state:State,kind:string}){
 const lines=state.lines.map(s=>s.trimEnd());
 const editing=lines.some(s=>s.trim()==='~')&&!lines.some(s=>s.includes('kinakaze $'));
 const written=kind==='vim'?lines.findIndex(s=>s.includes('written')):-1;
 const start=editing?0:written>=0?written:Math.max(0,state.y-12);
 const rows=editing?[...lines.slice(0,12),lines.at(-1)??'']:lines.slice(start,start+13);
 const cursorY=editing&&state.y===lines.length-1?12:state.y-start;
 const success=/written|JAVA_.*OK|COMPLETE|CONNECTED/;
 return <div className={`cli-screen ${kind==='java'?'java-screen':''}`}>
  {rows.map((line,i)=><div key={i} className={`cli-row ${success.test(line)?'verified-row':''} ${i===cursorY&&!state.hidden?'current-row':''} ${editing&&i===12?'editor-status':''}`}>
   <span>{line||' '}</span>
   {i===cursorY&&!state.hidden&&<span className="precise-cursor" style={{left:`${state.x}ch`}}/>}
  </div>)}
 </div>;
}
function CliFlow({local,evidence}:{local:number,evidence:Evidence}){
 const data=evidence.cli,commands=data.commands;
 const schedule=[{start:.18,duration:.65,output:.98},{start:2.05,duration:1.20,output:3.42},{start:3.65,duration:.58,output:4.46}];
 const phase=local<1.8?clamp(local/1.8):local<3.65?1+clamp((local-2.05)/1.6):2+clamp((local-4.2)/1.5);
 return <Shell title="管道与重定向" label="SHELL / FILES" steps={['查看文件','管道处理','读回结果']} phase={phase} local={local} cue={local<4.46?<><code>|</code> 连接命令　<code>&gt;</code> 写入文件</>:<>result.txt 已写入 · Windows 侧读回一致 <span className="cue-check">✓</span></>}>
  <div className="cli-screen pipeline-screen">
   {commands.map((entry:any,i:number)=>{
    const slot=schedule[i],text=typing(entry.command,local,slot.start,slot.duration),running=local>=slot.start&&local<slot.output;
    const output=(entry.stdout as string).trimEnd().split('\n').filter(Boolean);
    return <React.Fragment key={entry.command}>
     <div className="cli-row command-row" style={{visibility:local>=slot.start?'visible':'hidden'}}><span className="shell-prompt">kinakaze <b>$</b> </span><Command text={text}/>{running&&<span className="inline-cursor"/>}</div>
     {output.map((line:string,j:number)=><div key={j} className={`cli-row ${i===2?'pipeline-result':''}`} style={{opacity:ease((local-slot.output-j*.075)/.14),transform:`translateY(${(1-ease((local-slot.output-j*.075)/.14))*7}px)`}}>{line}</div>)}
    </React.Fragment>;
   })}
   <div className="cli-row command-row return-prompt" style={{opacity:ease((local-5.15)/.2)}}><span className="shell-prompt">kinakaze <b>$</b> </span><span className="inline-cursor"/></div>
   <div className="file-verified" style={{opacity:ease((local-5.45)/.25)}}><span>✓</span> result.txt <small>3 lines · exit 0</small></div>
  </div>
 </Shell>;
}

export function EvidenceScene({kind,local,t,res,tailFrames=0}:{kind:string,local:number,t:number,res:{evidence:Evidence,timeline:any},tailFrames?:number}){
 if(kind==='desktop'||kind==='minecraft'){
  const shot=res.timeline.shots.find((s:any)=>s.id===kind),desktop=kind==='desktop';
  return <><Sequence from={shot.from} durationInFrames={shot.to-shot.from+tailFrames}><AbsoluteFill><Video src={staticFile(desktop?'gnome.mp4':settings.minecraftClip)} trimBefore={desktop?180:300} muted objectFit="cover" style={{width:'100%',height:'100%'}}/></AbsoluteFill></Sequence><div className="demo-caption" style={{opacity:ease(local/.4),transform:`translateY(${(1-ease(local/.4))*12}px)`}}><span>{desktop?'GNOME 桌面':'Minecraft Java Edition'}</span><small>{desktop?'Linux 图形程序 · 实际运行画面':'Linux 客户端 · 主菜单实录'}</small></div></>;
 }
 if(kind==='cli')return <CliFlow local={local} evidence={res.evidence}/>;
 const data=res.evidence.terminals[kind];
 let progress=0,phase=0,cue:React.ReactNode;
 if(kind==='vi'){
  progress=(local<2.55?blend(0,130,(local-.18)/2.25):local<2.95?blend(130,139,(local-2.55)/.4):blend(139,142,(local-2.95)/.38))/240;
  phase=local<2.55?0:local<2.95?1:2;
  cue=local<2.55?<><kbd>i</kbd> 输入内容</>:local<2.95?<><kbd>Esc</kbd> 退出编辑模式</>:<><kbd>:wq</kbd> 保存并退出</>;
 }else if(kind==='vim'){
  progress=local<1.45?blend(0,.49,local/1.45):local<2.03?blend(.49,.56,(local-1.45)/.58):blend(.56,.88,(local-2.03)/.9);
  phase=local<1.45?0:local<2.15?1:2;
  cue=local<1.45?<><kbd>o</kbd> 追加一行</>:local<2.15?<><kbd>Esc</kbd><kbd>:wq</kbd> 保存并退出</>:<><code>cat /tmp/hello.txt</code> 读回保存内容</>;
 }else if(kind==='ssh'){
  progress=clamp((local-.2)/2.3);phase=local<.65?0:local<1.65?1:2;
  cue=<>Windows OpenSSH <span className="cue-arrow">→</span> Linux sshd · 实际连接结果</>;
 }else{
  progress=local<1.3?blend(0,.38,local/1.3):blend(.38,.85,(local-1.3)/1.25);phase=local<1.3?0:local<2.55?1:2;
  cue=local<2.5?<><code>java -version</code> <span className="cue-arrow">→</span> 运行 Java 程序</>:<>线程、文件与子进程检查通过 <span className="cue-check">✓</span></>;
 }
 const state=data[Math.min(data.length-1,Math.floor(progress*(data.length-1)))];
 const info:Record<string,{title:string,label:string,steps:string[]}>= {
  vi:{title:'vi 编辑文件',label:'VI / VIM',steps:['输入内容','退出编辑','保存文件']},
  vim:{title:'Vim 保存与读回',label:'VI / VIM',steps:['追加一行',':wq 保存','cat 读回']},
  ssh:{title:'SSH 连接到 Linux',label:'SSH / SSHD',steps:['连接','执行','返回结果']},
  java:{title:'Linux 版 Java，直接运行',label:'JAVA / RUNTIME',steps:['查看版本','运行程序','执行完成']}
 };
 return <Shell {...info[kind]} phase={phase} local={local} cue={cue} continuous={kind==='vim'||kind==='ssh'}><TerminalState state={state} kind={kind}/></Shell>;
}
