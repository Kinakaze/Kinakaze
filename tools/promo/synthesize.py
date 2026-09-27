"""Synthesize the promo's female narration using the user's local VITS model."""
import argparse
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
VITS = Path('E:/APPD/VITS/VITS-barbara')
sys.path.insert(0, str(VITS))
os.chdir(VITS)
import numpy as np
import torch
import scipy.io.wavfile as wavfile
import utils
import commons
from models import SynthesizerTrn
from text import text_to_sequence


def main():
    ap=argparse.ArgumentParser()
    ap.add_argument('--speed',type=float,default=1.12)
    ap.add_argument('--speaker',type=int,default=0)
    ap.add_argument('--config',type=Path,default=VITS/'configs/finetune_speaker.json')
    ap.add_argument('--checkpoint',type=Path,default=VITS/'pretrained_models/G_0.pth')
    ap.add_argument('--script',type=Path,default=ROOT/'tools/promo/script.json')
    ap.add_argument('--output',type=Path,default=ROOT/'artifacts/promo-v1/audio')
    args=ap.parse_args()
    torch.set_num_threads(4)
    torch.manual_seed(20260926)
    device='cuda:0' if torch.cuda.is_available() else 'cpu'
    config=args.config
    checkpoint=args.checkpoint
    hps=utils.get_hparams_from_file(str(config))
    model=SynthesizerTrn(len(hps.symbols),hps.data.filter_length//2+1,
                         hps.train.segment_size//hps.data.hop_length,
                         n_speakers=hps.data.n_speakers,**hps.model).to(device).eval()
    utils.load_checkpoint(str(checkpoint),model,None)
    script=json.loads(args.script.read_text(encoding='utf-8'))
    output=args.output
    output.mkdir(parents=True,exist_ok=True)
    records=[]
    for segment in script['segments']:
        start=time.monotonic()
        # The local front end prints phonemes; keep those out of terminal output.
        with contextlib.redirect_stdout(io.StringIO()):
            seq=text_to_sequence(segment['speak'],hps.symbols,hps.data.text_cleaners)
        if hps.data.add_blank:
            seq=commons.intersperse(seq,0)
        x=torch.LongTensor(seq).unsqueeze(0).to(device)
        length=torch.LongTensor([len(seq)]).to(device)
        sid=torch.LongTensor([args.speaker]).to(device)
        with torch.no_grad():
            audio=model.infer(x,length,sid=sid,noise_scale=.48,noise_scale_w=.55,
                              length_scale=1/args.speed)[0][0,0].cpu().float().numpy()
        # Limit gain without hard clipping, retaining short model pauses.
        peak=float(np.max(np.abs(audio)))
        if peak>0:
            audio=audio*(.86/peak)
        path=output/(segment['id']+'.wav')
        wavfile.write(str(path),hps.data.sampling_rate,(np.clip(audio,-1,1)*32767).astype(np.int16))
        record=dict(segment, audio=str(path), audio_seconds=round(len(audio)/hps.data.sampling_rate,6))
        records.append(record)
        print(segment['id'],record['audio_seconds'],'seconds; generated in',round(time.monotonic()-start,2),flush=True)
    manifest=dict(title=script['title'],style=script['style'],engine='VITS / VITS-fast-fine-tuning',
                  model=str(checkpoint),model_sha256=hashlib.sha256(checkpoint.read_bytes()).hexdigest(),
                  config=str(config),speaker_id=args.speaker,speakers=json.loads(config.read_text(encoding='utf-8')).get('speakers'),sample_rate=hps.data.sampling_rate,
                  speed=args.speed,device=device,segments=records)
    (output/'manifest.json').write_text(json.dumps(manifest,ensure_ascii=False,indent=2),encoding='utf-8')
    print('TOTAL SPOKEN',round(sum(x['audio_seconds'] for x in records),2),flush=True)


if __name__=='__main__':main()
