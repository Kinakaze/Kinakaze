"""Author four original editable Blender scenes, GLB assets and Cycles lookdev renders."""
import bpy, math, random, json, sys
from pathlib import Path
from mathutils import Vector

ROOT=Path(__file__).resolve().parents[3]
OUT=ROOT/'artifacts/promo-lab'
PUB=OUT/'public'
for p in [PUB/'models',PUB/'lookdev',OUT/'blender',OUT/'reports']:
    p.mkdir(parents=True,exist_ok=True)
random.seed(260926)

def mat(name,col,metal=0,rough=.3,trans=0):
    m=bpy.data.materials.new(name);m.diffuse_color=(*col,1);m.use_nodes=True
    p=m.node_tree.nodes.get('Principled BSDF');p.inputs['Base Color'].default_value=(*col,1)
    p.inputs['Metallic'].default_value=metal;p.inputs['Roughness'].default_value=rough
    p.inputs['Coat Weight'].default_value=.4;p.inputs['Coat Roughness'].default_value=.17
    p.inputs['Transmission Weight'].default_value=trans
    return m

def empty(name,loc=(0,0,0),parent=None):
    o=bpy.data.objects.new(name,None);bpy.context.collection.objects.link(o);o.location=loc;o.parent=parent
    return o

def finish(o,name,loc,ma,parent=None,bevel=0):
    o.name=name;o.location=loc;o.parent=parent
    if ma:o.data.materials.append(ma)
    if bevel:
        m=o.modifiers.new('Machined edge','BEVEL');m.width=bevel;m.segments=3
        n=o.modifiers.new('Weighted normals','WEIGHTED_NORMAL');n.keep_sharp=True
    if o.type=='MESH':
        for p in o.data.polygons:p.use_smooth=True
    return o

def cube(name,loc,dims,ma,parent=None,bevel=.045):
    bpy.ops.mesh.primitive_cube_add()
    o=bpy.context.object;o.dimensions=dims
    bpy.ops.object.transform_apply(location=False,rotation=False,scale=True)
    return finish(o,name,loc,ma,parent,bevel)

def cyl(name,loc,r,depth,ma,parent=None,verts=64):
    bpy.ops.mesh.primitive_cylinder_add(vertices=verts,radius=r,depth=depth)
    return finish(bpy.context.object,name,loc,ma,parent,.035)

def torus(name,loc,r,t,ma,parent=None,arc=2*math.pi):
    if arc>=2*math.pi:
        bpy.ops.mesh.primitive_torus_add(major_segments=96,minor_segments=10,major_radius=r,minor_radius=t)
        return finish(bpy.context.object,name,loc,ma,parent)
    return tube(name,[(r*math.cos(i*arc/80),r*math.sin(i*arc/80),0) for i in range(81)],t,ma,parent,loc)

def tube(name,points,r,ma,parent=None,loc=(0,0,0)):
    cv=bpy.data.curves.new(name,'CURVE');cv.dimensions='3D';cv.resolution_u=2;cv.bevel_depth=r;cv.bevel_resolution=3
    s=cv.splines.new('POLY');s.points.add(len(points)-1)
    for p,v in zip(s.points,points):p.co=(*v,1)
    o=bpy.data.objects.new(name,cv);bpy.context.collection.objects.link(o);o.location=loc;o.parent=parent;o.data.materials.append(ma)
    bpy.context.view_layer.objects.active=o;o.select_set(True);bpy.ops.object.convert(target='MESH');o.select_set(False)
    return o

def join_children(group):
    obs=[o for o in list(group.children) if o.type=='MESH']
    if not obs:return
    bpy.ops.object.select_all(action='DESELECT')
    for o in obs:
        o.select_set(True);bpy.context.view_layer.objects.active=o
        for mod in list(o.modifiers):bpy.ops.object.modifier_apply(modifier=mod.name)
    bpy.context.view_layer.objects.active=obs[0];bpy.ops.object.join();obs[0].name=group.name+'_mesh'
    bpy.ops.object.select_all(action='DESELECT')

def gear(name,loc,r,ma,teeth=18,parent=None):
    g=empty(name,loc,parent)
    cyl('bearing',(0,0,0),r*.7,.16,ma,g)
    torus('rim',(0,0,.025),r*.76,r*.11,ma,g)
    for i in range(teeth):
        a=i*math.tau/teeth
        o=cube('tooth',(math.cos(a)*r*.94,math.sin(a)*r*.94,0),(r*.26,r*.19,.17),ma,g,.02);o.rotation_euler.z=a
    cyl('hub',(0,0,.11),r*.18,.2,SILVER,g)
    cyl('inset',(0,0,.22),r*.065,.025,BLUE,g)
    join_children(g);return g

def monogram(name,loc,scale=1,parent=None):
    g=empty(name,loc,parent)
    # Sculpted K, raised in the X/Z plane, retaining substantial side faces.
    cube('stem',(-.67,0,0),(.31,.36,2.4),BLUE,g,.10)
    for sign in [-1,1]:
        o=cube('arm',(.12,0,sign*.56),(.34,.36,1.62),BLUE,g,.10);o.rotation_euler.y=sign*.77
    join_children(g);g.scale=(scale,)*3;return g

def bloom():
    root=empty('bloom')
    cyl('foundation',(0,0,-.15),3.8,.2,WHITE,root)
    torus('etched_edge',(0,0,-.035),3.57,.018,SILVER,root)
    for i in range(5):
        a=i*math.tau/5+.4;radius=2.5
        loc=(math.cos(a)*radius,math.sin(a)*radius,.12)
        gear('spin_%02d'%i,loc,.62,BLUE if i%2==0 else SILVER,20,root)
        lift=empty('rise_%02d'%i,(loc[0],loc[1],.17),root)
        for j in range(4):
            cyl('spool',(0,0,j*.2+.1),.43-j*.045,.12,WHITE if j%2 else ICE,lift)
        cyl('blue_terminal',(0,0,.89),.22,.10,BLUE,lift);join_children(lift)
    center=empty('rise_08',(0,0,.1),root)
    for j in range(5):
        cyl('stack',(0,0,.16+j*.19),1.06-j*.09,.13,WHITE if j%2 else BLUE,center)
    join_children(center)
    gear('spin_09',(0,0,.14),1.38,SILVER,32,root)
    for i in range(12):
        a=i*math.tau/12
        g=empty('petal_%02d'%i,(math.cos(a)*1.21,math.sin(a)*1.21,.45),root);g.rotation_euler.z=a
        # Fluted radial shell, carefully beveled, with blue edge inlay.
        o=cube('petal',(0.64,0,.25),(1.38,.29,.11),WHITE,g,.09);o.rotation_euler.y=-.4
        o=cube('inlay',(.68,0,.31),(1.1,.06,.018),BLUE,g,.02);o.rotation_euler.y=-.4
        join_children(g)
    m=monogram('brand',(0,0,2.03),.52,root)
    for i in range(3):
        o=torus('orbital_%d'%i,(0,0,1.7),1.05+i*.17,.018,ICE,root,math.pi*1.5);o.rotation_euler=(.3+i*.45,.35,i*.8)
    return root

def prism():
    root=empty('prism')
    monogram('brand',(0,0,1.85),1.28,root)
    # Ceramic architectural plinth; stepped bevels create controlled highlights.
    for i in range(3):
        o=cube('plinth',(0,.15,-.12-i*.17),(3.8+i*.15,2.3+i*.15,.13),WHITE if i!=1 else SILVER,root,.14)
    for i in range(7):
        g=empty('wafer_%02d'%i,((i-3)*.38,1.15,1.75),root)
        cube('wafer',(0,0,0),(.09,1.05,2.95-abs(i-3)*.26),ICE if i%2==0 else WHITE,g,.055);join_children(g)
    # A flowing enamel ribbon wraps the symbol; mesh is authored in Blender.
    for n in range(2):
        pts=[]
        for j in range(150):
            a=j/149*math.pi*1.6-.9+n*math.pi
            pts.append((math.cos(a)*(2.22+n*.24),math.sin(a)*1.55,1.6+math.sin(a*1.4)*.65))
        tube('ribbon_%d'%n,pts,.047 if n else .075,BLUE if n else SILVER,root)
    for i in range(30):
        a=i*math.tau/30
        o=cube('tick',(math.cos(a)*2.8,math.sin(a)*2.2,-.25),(.055,.12,.025),SILVER,root,.008);o.rotation_euler.z=a
    return root

def flow():
    root=empty('flow')
    for i in range(11):
        g=empty('gate_%02d'%i,(0,(i-5)*1.18,2.08),root)
        o=torus('portal',(0,0,0),1.74,.14,WHITE if i%3 else ICE,g);o.rotation_euler.x=math.pi/2
        o=torus('edge',(0,-.02,0),1.9,.023,BLUE,g,math.pi*1.56);o.rotation_euler.x=math.pi/2;o.rotation_euler.y=i*.16
        for j in range(3):
            a=j*math.tau/3+i*.3
            o=cube('fin',(math.cos(a)*2.1,0,math.sin(a)*2.1),(.33,.11,.065),SILVER,g,.025);o.rotation_euler.y=-a
        join_children(g)
    for side in [-1,1]:
        pts=[(side*(.76+.14*math.sin(j*.14)),j*.15-7,.3) for j in range(96)]
        tube('rail',pts,.035,BLUE,root)
    # Small architectural fins catch light as the camera flies past.
    for i in range(26):
        y=i*.48-6
        for side in [-1,1]:cube('foundation_fin',(side*2.55,y,-.02),(.8,.10,.18),WHITE,root,.025)
    monogram('brand',(0,7.1,2.15),.78,root)
    return root

def voxel():
    root=empty('voxel')
    for x in range(-7,8):
        for y in range(-6,7):
            dist=math.sqrt((x/1.15)**2+y*y)
            if dist>6.8:continue
            h=.35+1.0*math.exp(-dist*.2)+.5*math.sin(x*.63)*math.cos(y*.51)
            g=empty('tile_%d_%d'%(x+7,y+6),(x*.48,y*.48,0),root)
            cube('column',(0,0,h/2),(.435,.435,h),BLUE if (x+y)%11==0 else (ICE if (x-y)%4==0 else WHITE),g,.034)
            join_children(g)
    monogram('brand',(0,0,2.6),.88,root)
    for i in range(3):
        o=torus('horizon_%d'%i,(0,0,.1),3.6+i*.14,.019,BLUE if i==1 else SILVER,root,math.pi*1.68);o.rotation_euler.z=i*.3
    return root

def aim(o,target):o.rotation_euler=(Vector(target)-o.location).to_track_quat('-Z','Y').to_euler()
def light(name,loc,power,size,color):
    d=bpy.data.lights.new(name,'AREA');d.energy=power;d.shape='DISK';d.size=size;d.color=color
    o=bpy.data.objects.new(name,d);bpy.context.collection.objects.link(o);o.location=loc;aim(o,(0,0,1));return o

selected=sys.argv[sys.argv.index('--')+1:] if '--' in sys.argv else []
selected=selected or ['bloom','prism','flow','voxel']
reports=[]
for name in selected:
    bpy.ops.object.select_all(action='SELECT');bpy.ops.object.delete(use_global=False)
    WHITE=mat('Porcelain',(.82,.875,.91),.08,.26)
    BLUE=mat('Azure enamel',(.012,.14,.54),.48,.19)
    SILVER=mat('Brushed aluminium',(.36,.48,.59),.82,.27)
    ICE=mat('Ice resin',(.19,.59,.87),.25,.19,.24)
    root=globals()[name]()
    bpy.ops.object.select_all(action='DESELECT')
    for o in bpy.context.scene.objects:o.select_set(True)
    bpy.ops.export_scene.gltf(filepath=str(PUB/'models'/(name+'.glb')),export_format='GLB',use_selection=True,export_apply=True,export_yup=True,export_extras=True)
    floor=cube('Studio_floor',(0,0,-.42),(200,200,.12),WHITE,None,.04)
    light('Large softbox',(-5,-6,9),1600,7,(.88,.95,1))
    light('Blue rim',(5,2,7),2100,5,(.55,.76,1))
    light('White reflection strip',(-2,5,6),1900,4,(1,1,1))
    world=bpy.data.worlds.new('Ice studio');world.use_nodes=True;world.node_tree.nodes['Background'].inputs[0].default_value=(.72,.82,.91,1);world.node_tree.nodes['Background'].inputs[1].default_value=.35
    scene=bpy.context.scene;scene.world=world
    bpy.ops.object.camera_add();cam=bpy.context.object;cam.name='Camera';scene.camera=cam
    camera={'bloom':((7,-9,7.5),(0,0,.7),52),'prism':((6,-9,5.7),(0,0,1.35),62),'flow':((5,-11,5),(0,0,1.5),42),'voxel':((7,-9,7),(0,0,1),54)}[name]
    cam.location=camera[0];aim(cam,camera[1]);cam.data.lens=camera[2]
    cam.data.dof.use_dof=True;cam.data.dof.focus_distance=(Vector(camera[1])-cam.location).length;cam.data.dof.aperture_fstop=7
    scene.render.engine='CYCLES';scene.cycles.samples=32;scene.cycles.use_denoising=True
    try:
        prefs=bpy.context.preferences.addons['cycles'].preferences;prefs.compute_device_type='OPTIX';prefs.get_devices()
        for d in prefs.devices:d.use=d.type=='OPTIX'
        scene.cycles.device='GPU'
    except Exception:pass
    scene.render.resolution_x=1920;scene.render.resolution_y=1080;scene.render.resolution_percentage=100
    scene.render.image_settings.file_format='PNG';scene.render.filepath=str(PUB/'lookdev'/(name+'.png'))
    scene.view_settings.view_transform='AgX';scene.view_settings.look='AgX - Medium High Contrast'
    scene.render.fps=60;scene.frame_end=840
    # Editable camera demonstration is stored in each .blend.
    cam.keyframe_insert(data_path='location',frame=1);cam.keyframe_insert(data_path='rotation_euler',frame=1)
    cam.location.x+=1.1;cam.location.z-=.35;aim(cam,camera[1]);cam.keyframe_insert(data_path='location',frame=840);cam.keyframe_insert(data_path='rotation_euler',frame=840)
    scene.frame_set(1)
    bpy.ops.wm.save_as_mainfile(filepath=str(OUT/'blender'/(name+'.blend')))
    bpy.ops.render.render(write_still=True)
    reports.append({'name':name,'objects':len(scene.objects),'blender':bpy.app.version_string,'renderer':scene.render.engine,'device':scene.cycles.device,'asset':str(PUB/'models'/(name+'.glb'))})
    print('ASSET_DONE',name,flush=True)
(OUT/'reports'/'blender-assets.json').write_text(json.dumps(reports,indent=2),encoding='utf-8')
