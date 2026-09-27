import {bundle} from '@remotion/bundler';
import {getCompositions,openBrowser,renderMedia,renderStill} from '@remotion/renderer';
import fs from 'node:fs';import path from 'node:path';import {fileURLToPath} from 'node:url';
const dir=path.dirname(fileURLToPath(import.meta.url));const root=path.resolve(dir,'../../..');const out=path.join(root,'artifacts/promo-amv');
const settings=JSON.parse(fs.readFileSync(path.join(dir,'amv-settings.json'),'utf8'));
fs.mkdirSync(path.join(out,settings.stillsDirectory),{recursive:true});
const mode=process.argv[2]??'stills';
const serveUrl=await bundle({entryPoint:path.join(dir,'src/amv.tsx'),publicDir:path.join(out,'public'),outDir:path.join(out,'bundle')});
const browser=await openBrowser('chrome',{browserExecutable:'C:/Users/nanaeo/AppData/Local/ms-playwright/chromium-1223/chrome-win64/chrome.exe',chromiumOptions:{gl:'angle'},logLevel:'warn'});
const errors=[],drawingBuffers=new Set();const common={serveUrl,puppeteerInstance:browser,timeoutInMilliseconds:90000,onBrowserLog:e=>{if(e.type==='error'){errors.push(e.text);console.log('BROWSER_ERROR',e.text)}if(e.text?.startsWith('AMV_DRAWING_BUFFER'))drawingBuffers.add(e.text)}};
try{
 const [composition]=await getCompositions(serveUrl,{puppeteerInstance:browser,timeoutInMilliseconds:90000});
 if(mode==='stills'||mode==='ending-stills'){
  const times=mode==='ending-stills'?[44.15,44.17,44.57,45,47.2,49.9,49.92,50.32,50.75,52.5]:[8.4,12.5,14.2,16.5,17.2,18.6,20.4,22.3,24.5,27.3,29.5,31.8,33.2,36.3,38.0,41.2,45.2,47.2,51.7];
  for(const sec of times){await renderStill({...common,composition,frame:Math.round(sec*settings.fps),output:path.join(out,settings.stillsDirectory,`amv-${sec}.png`)});console.log('STILL',sec);}
 }else{
  let last=-1;await renderMedia({...common,composition,outputLocation:path.join(out,mode==='test'?`amv-${settings.reportSuffix}-cli-preview-2k.mp4`:settings.filename),codec:'h264',crf:settings.crf,imageFormat:settings.captureFormat,pixelFormat:'yuv420p',audioBitrate:'320k',concurrency:3,...(mode==='test'?{frameRange:[691,1151]}:{}),onProgress:p=>{const step=Math.floor(p.progress*20)*5;if(step!==last){last=step;console.log('PROGRESS',step,p.renderedFrames)}}});
 }
}finally{await browser.close({silent:true});fs.writeFileSync(path.join(out,'reports',`render-${mode}-${settings.reportSuffix}.json`),JSON.stringify({mode,width:settings.width,height:settings.height,captureFormat:settings.captureFormat,drawingBuffers:[...drawingBuffers],errors,finished:new Date().toISOString()},null,2));}
