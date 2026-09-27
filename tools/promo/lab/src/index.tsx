import React from 'react';
import {Composition,registerRoot} from 'remotion';
import {Study} from './study';
import './style.css';

const studies=['bloom','prism','flow','voxel'] as const;
const Root=()=> <>{studies.map((kind,i)=><Composition key={kind} id={`${'ABCD'[i]}-${kind}`} component={Study} width={1920} height={1080} fps={60} durationInFrames={824} defaultProps={{kind}} />)}</>;
registerRoot(Root);
