"""Render a real Blender camera shot used at the start of study B."""
import bpy,math,json,time
from pathlib import Path
from mathutils import Vector
ROOT=Path(__file__).resolve().parents[3];OUT=ROOT/'artifacts/promo-lab'
bpy.ops.wm.open_mainfile(filepath=str(OUT/'blender/prism.blend'))
scene=bpy.context.scene;cam=scene.camera
scene.render.engine='BLENDER_EEVEE';scene.render.resolution_x=1920;scene.render.resolution_y=1080
scene.render.resolution_percentage=100;scene.render.fps=30
scene.render.image_settings.file_format='PNG'
scene.frame_start=1;scene.frame_end=124
cam.animation_data_clear();cam.data.lens=62
cam.data.dof.use_dof=True;cam.data.dof.aperture_fstop=4.5
frames=OUT/'render/blender-macro';frames.mkdir(parents=True,exist_ok=True)
def ease(x):return x*x*(3-2*x)
started=time.time()
for f in range(1,125):
    t=(f-1)/123;p=ease(t)
    cam.location=(3.1+2.9*p,-4.3-4.7*p,2.9+2.8*p)
    target=Vector((0,0,1.6));cam.rotation_euler=(target-cam.location).to_track_quat('-Z','Y').to_euler()
    cam.data.dof.focus_distance=(target-cam.location).length
    scene.frame_set(f)
    for o in scene.objects:
        if o.name.startswith('wafer_') and o.type=='EMPTY':
            i=int(o.name.split('_')[1]);o.location.x=(i-3)*(.38+.17*math.sin(t*math.pi));o.location.y=1.15;o.rotation_euler.y=(i-3)*.025*math.sin(t*math.pi)
    scene.render.filepath=str(frames/('%04d.png'%f));bpy.ops.render.render(write_still=True)
    if f%15==0:print('MACRO_PROGRESS',f,124,round(time.time()-started,1),flush=True)
scene.frame_set(1)
bpy.ops.wm.save_as_mainfile(filepath=str(OUT/'blender/prism-macro.blend'))
(OUT/'reports/blender-macro.json').write_text(json.dumps({'frames':124,'fps':30,'engine':scene.render.engine,'seconds':time.time()-started},indent=2),encoding='utf-8')
