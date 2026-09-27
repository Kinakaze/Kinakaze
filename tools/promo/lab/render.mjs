import {bundle} from '@remotion/bundler';
import {getCompositions,renderMedia,renderStill,openBrowser} from '@remotion/renderer';
import path from 'node:path';
import fs from 'node:fs';
import {fileURLToPath} from 'node:url';

const dir=path.dirname(fileURLToPath(import.meta.url));
const root=path.resolve(dir,'../../..');
const out=path.join(root,'artifacts/promo-lab');
const mode=process.argv[2]??'stills';
const only=process.argv[3];
const chrome='C:/Users/nanaeo/AppData/Local/ms-playwright/chromium-1223/chrome-win64/chrome.exe';
for(const sub of ['stills','reports','render'])fs.mkdirSync(path.join(out,sub),{recursive:true});
const serveUrl=await bundle({entryPoint:path.join(dir,'src/index.tsx'),publicDir:path.join(out,'public'),outDir:path.join(out,'bundle'),onProgress:p=>{if(p===100)console.log('BUNDLE_READY')}});
const browser=await openBrowser('chrome',{browserExecutable:chrome,chromiumOptions:{gl:'angle'},logLevel:'warn'});
const logs=[],gpus=new Set(),completed=[];
const shared={serveUrl,puppeteerInstance:browser,chromiumOptions:{gl:'angle'},timeoutInMilliseconds:90000,onBrowserLog:e=>{if(e.type==='error'){logs.push(e.text);console.log('BROWSER_ERROR',e.text)}if(e.text?.startsWith('KINAKAZE_GPU')){gpus.add(e.text);console.log(e.text)}}};
const comps=await getCompositions(serveUrl,{puppeteerInstance:browser,timeoutInMilliseconds:90000});
try{
 for(const composition of comps.filter(c=>!only||c.id.startsWith(only))){
  console.log('START',composition.id,mode);
  const started=Date.now();
  if(mode==='stills'){
   for(const f of [90,270,450,700]){
    await renderStill({...shared,composition,frame:f,output:path.join(out,'stills',`${composition.id}-${f}.png`),imageFormat:'png'});
    console.log('STILL',composition.id,f);
   }
  }else{
   let last=-1;
   await renderMedia({...shared,composition,codec:'h264',outputLocation:path.join(out,`${composition.id}.mp4`),crf:17,pixelFormat:'yuv420p',audioBitrate:'256k',imageFormat:'jpeg',jpegQuality:96,concurrency:2,
    ...(mode==='test'?{frameRange:[0,179]}:{}),
    onProgress:p=>{const progress=Math.floor(p.progress*20)*5;if(progress!==last){last=progress;console.log('PROGRESS',composition.id,progress,p.renderedFrames)}}});
  }
  console.log('DONE',composition.id,Math.round((Date.now()-started)/1000));
  completed.push(composition.id);
 }
}finally{
 await browser.close({silent:true});
 fs.writeFileSync(path.join(out,'reports',`remotion-${mode}-${only??'all'}.json`),JSON.stringify({remotion:'4.0.529',mode,errors:logs,gpus:[...gpus],completed,completedAt:new Date().toISOString()},null,2));
}
