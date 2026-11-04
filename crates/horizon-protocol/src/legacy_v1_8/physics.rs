//! Early classic (pre-1.9) player motion and combat-input primitives.
//!
//! Collision is intentionally supplied by the caller: pre-flattening 1.8
//! blocks have many non-cube shapes. Do not treat every non-air ID as a cube.
//! These functions do not register hits: an online server remains authoritative.
//! Numerical constants follow the classic 20 TPS model where applicable; this
//! is not yet a byte-for-byte parity implementation of vanilla 1.8.9 physics.
use crate::{Error, Result};
use super::{inventory, play};

pub const TICKS_PER_SECOND: u32 = 20;
pub const PLAYER_WIDTH: f64 = 0.6;
pub const PLAYER_HEIGHT: f64 = 1.8;
pub const STEP_HEIGHT: f64 = 0.6;
pub const GRAVITY_PER_TICK: f64 = 0.08;
pub const VERTICAL_DRAG: f64 = 0.98;
pub const JUMP_VELOCITY: f64 = 0.42;
pub const SPRINT_SPEED_MULTIPLIER: f64 = 1.3;

#[derive(Clone,Copy,Debug,Default,PartialEq)]
pub struct Vec3 {pub x:f64,pub y:f64,pub z:f64}
impl Vec3 {
    pub fn new(x:f64,y:f64,z:f64)->Self{Self{x,y,z}}
    fn add(self,rhs:Self)->Self{Self::new(self.x+rhs.x,self.y+rhs.y,self.z+rhs.z)}
}
#[derive(Clone,Copy,Debug,PartialEq)]
pub struct Aabb {pub min:Vec3,pub max:Vec3}
impl Aabb {
    pub fn block(x:i32,y:i32,z:i32)->Self {Self {
        min:Vec3::new(f64::from(x),f64::from(y),f64::from(z)),
        max:Vec3::new(f64::from(x)+1.0,f64::from(y)+1.0,f64::from(z)+1.0),
    }}
    pub fn player(feet:Vec3)->Self {let h=PLAYER_WIDTH/2.0;Self{
        min:Vec3::new(feet.x-h,feet.y,feet.z-h),
        max:Vec3::new(feet.x+h,feet.y+PLAYER_HEIGHT,feet.z+h),
    }}
    fn shifted(self,v:Vec3)->Self{Self{min:self.min.add(v),max:self.max.add(v)}}
}
/// Return actual world-space collision boxes for relevant 1.8 block states.
/// A fence, slab, fluid, trapdoor etc. must NOT be modeled as a full cube.
pub trait CollisionWorld {
    fn boxes(&self, swept: Aabb) -> Vec<Aabb>;
}
#[derive(Clone,Copy,Debug,Default,PartialEq)]
pub struct Controls {
    /// -1..1, relative to camera orientation.
    pub forward:f64,
    pub strafe:f64,
    pub jump:bool,
    pub sprint:bool,
    pub sneak:bool,
}
#[derive(Clone,Copy,Debug,PartialEq)]
pub struct ClassicPlayer {
    /// Feet position, as transmitted in protocol 47 PlayerPosition.
    pub feet:Vec3,
    pub velocity:Vec3,
    pub yaw:f64,
    pub pitch:f64,
    pub on_ground:bool,
    pub sprinting:bool,
    /// True while right click / long-press holds the sword in use.
    pub using_sword:bool,
}
impl ClassicPlayer {
    pub fn new(x:f64,y:f64,z:f64)->Self{Self{
        feet:Vec3::new(x,y,z),velocity:Vec3::default(),
        yaw:0.0,pitch:0.0,on_ground:false,sprinting:false,using_sword:false,
    }}
    pub fn bbox(&self)->Aabb{Aabb::player(self.feet)}
    pub fn apply_server_position(&mut self,x:f64,y:f64,z:f64,yaw:f64,pitch:f64)->Result<()> {
        if ![x,y,z,yaw,pitch].into_iter().all(f64::is_finite) {
            return Err(Error::new("nonfinite legacy server correction"));
        }
        self.feet=Vec3::new(x,y,z);self.yaw=yaw;self.pitch=pitch;
        self.velocity=Vec3::default();self.on_ground=false;Ok(())
    }
    /// Server-authoritative velocity (protocol 47 packet 0x12) should be applied
    /// to this state; local knockback prediction must not replace it.
    pub fn apply_server_velocity(&mut self,x:f64,y:f64,z:f64)->Result<()> {
        if ![x,y,z].into_iter().all(f64::is_finite) {
            return Err(Error::new("nonfinite legacy server velocity"));
        }
        self.velocity=Vec3::new(x,y,z);Ok(())
    }
    /// Send one attack packet per actual user action, with NO 1.9+ cooldown.
    /// 1.8 sprint reset happens when an attack consumes sprint knockback.
    pub fn attack(&mut self,entity_id:i32)->Result<Vec<u8>> {
        let bytes=play::use_entity_body(entity_id,play::UseEntityAction::Attack)?;
        self.sprinting=false;
        Ok(bytes)
    }
    /// Begin sword blocking using C08 use-item with sentinel (-1,-1,-1).
    pub fn sword_block(&mut self,item:Option<&inventory::ItemStack>)->Result<Vec<u8>> {
        let bytes=play::sword_block_body(item)?;
        self.using_sword=true;
        Ok(bytes)
    }
    /// Finish sword blocking using C07 release-use-item (status 5).
    pub fn sword_release(&mut self)->Vec<u8> {
        self.using_sword=false;
        play::release_use_item_body()
    }
    /// Update one 50ms tick. Inputs produce local movement prediction only;
    /// player position corrections from a server must override this state.
    pub fn tick(&mut self,controls:Controls,world:&impl CollisionWorld)->Result<()> {
        if ![controls.forward,controls.strafe,self.yaw,self.pitch].into_iter().all(f64::is_finite) {
            return Err(Error::new("nonfinite legacy controls"));
        }
        let mut forward=controls.forward.clamp(-1.0,1.0);
        let mut strafe=controls.strafe.clamp(-1.0,1.0);
        let len=(forward*forward+strafe*strafe).sqrt();
        if len>1.0 {forward/=len;strafe/=len;}
        self.sprinting=controls.sprint && forward>0.0 && !controls.sneak;
        let speed=if self.sprinting {SPRINT_SPEED_MULTIPLIER} else {1.0};
        let sneak=if controls.sneak {0.3} else {1.0};
        // Vanilla rotates local forward/strafe vectors around the view yaw.
        let angle=(self.yaw.to_radians()+90.0).sin_cos();
        let (s,c)=angle;
        let accel=if self.on_ground {0.1} else {0.02};
        self.velocity.x+=(forward*c+strafe*s)*accel*speed*sneak;
        self.velocity.z+=(forward*s-strafe*c)*accel*speed*sneak;
        if self.on_ground && controls.jump {self.velocity.y=JUMP_VELOCITY;}
        self.velocity.y-=GRAVITY_PER_TICK;
        let moved=collide(world,self.bbox(),self.velocity,STEP_HEIGHT,self.on_ground);
        self.feet=self.feet.add(moved.displacement);
        self.on_ground=moved.grounded;
        if moved.blocked_x {self.velocity.x=0.0;}
        if moved.blocked_y {self.velocity.y=0.0;}
        if moved.blocked_z {self.velocity.z=0.0;}
        let horizontal_drag=if self.on_ground {0.546} else {0.91};
        self.velocity.x*=horizontal_drag;
        self.velocity.z*=horizontal_drag;
        self.velocity.y*=VERTICAL_DRAG;
        Ok(())
    }
}
#[derive(Debug,Clone,Copy,PartialEq)]
pub struct Movement {
    pub displacement:Vec3,
    pub grounded:bool,
    pub blocked_x:bool,pub blocked_y:bool,pub blocked_z:bool,
}
fn overlap(a0:f64,a1:f64,b0:f64,b1:f64)->bool{a1>b0 && a0<b1}
/// Axis-aligned sweep on the boxes returned by the world collision provider.
fn sweep(start:Aabb,wanted:Vec3,blocks:&[Aabb])->Movement {
    let mut a=start;
    let mut d=wanted;
    // Classic collision order: Y, X, Z. Clamp independently against solids.
    for block in blocks {
        if overlap(a.min.x,a.max.x,block.min.x,block.max.x)
            && overlap(a.min.z,a.max.z,block.min.z,block.max.z) {
            if d.y>0.0 && a.max.y<=block.min.y {
                d.y=d.y.min(block.min.y-a.max.y);
            } else if d.y<0.0 && a.min.y>=block.max.y {
                d.y=d.y.max(block.max.y-a.min.y);
            }
        }
    }
    a=a.shifted(Vec3::new(0.0,d.y,0.0));
    for block in blocks {
        if overlap(a.min.y,a.max.y,block.min.y,block.max.y)
            && overlap(a.min.z,a.max.z,block.min.z,block.max.z) {
            if d.x>0.0 && a.max.x<=block.min.x {
                d.x=d.x.min(block.min.x-a.max.x);
            } else if d.x<0.0 && a.min.x>=block.max.x {
                d.x=d.x.max(block.max.x-a.min.x);
            }
        }
    }
    a=a.shifted(Vec3::new(d.x,0.0,0.0));
    for block in blocks {
        if overlap(a.min.x,a.max.x,block.min.x,block.max.x)
            && overlap(a.min.y,a.max.y,block.min.y,block.max.y) {
            if d.z>0.0 && a.max.z<=block.min.z {
                d.z=d.z.min(block.min.z-a.max.z);
            } else if d.z<0.0 && a.min.z>=block.max.z {
                d.z=d.z.max(block.max.z-a.min.z);
            }
        }
    }
    const EPS:f64=1e-8;
    Movement {
        displacement:d,
        grounded:wanted.y<0.0 && d.y>wanted.y+EPS,
        blocked_x:(d.x-wanted.x).abs()>EPS,
        blocked_y:(d.y-wanted.y).abs()>EPS,
        blocked_z:(d.z-wanted.z).abs()>EPS,
    }
}
pub fn collide(world:&impl CollisionWorld,box_at_feet:Aabb,wanted:Vec3,step:f64,grounded:bool)->Movement {
    // Prevent pathological allocations for untrusted or accidental huge moves.
    let desired=Vec3::new(wanted.x.clamp(-5.0,5.0),
        wanted.y.clamp(-5.0,5.0),wanted.z.clamp(-5.0,5.0));
    let min=Vec3::new(
        box_at_feet.min.x.min(box_at_feet.min.x+desired.x)-step,
        box_at_feet.min.y.min(box_at_feet.min.y+desired.y)-step,
        box_at_feet.min.z.min(box_at_feet.min.z+desired.z)-step,
    );
    let max=Vec3::new(
        box_at_feet.max.x.max(box_at_feet.max.x+desired.x)+step,
        box_at_feet.max.y.max(box_at_feet.max.y+desired.y)+step,
        box_at_feet.max.z.max(box_at_feet.max.z+desired.z)+step,
    );
    let blocks=world.boxes(Aabb{min,max});
    let ordinary=sweep(box_at_feet,desired,&blocks);
    if step<=0.0 || !grounded || (!ordinary.blocked_x&&!ordinary.blocked_z) {
        return ordinary;
    }
    // A raised sweep can navigate a block with a <=0.6-high collision box.
    let mut above=sweep(box_at_feet,Vec3::new(0.0,step,0.0),&blocks);
    let elevated=box_at_feet.shifted(above.displacement);
    let lateral=sweep(elevated,Vec3::new(desired.x,0.0,desired.z),&blocks);
    let changed=elevated.shifted(lateral.displacement);
    let down=sweep(changed,Vec3::new(0.0,desired.y-above.displacement.y,0.0),&blocks);
    above.displacement=above.displacement.add(lateral.displacement).add(down.displacement);
    let old_sq=ordinary.displacement.x.powi(2)+ordinary.displacement.z.powi(2);
    let new_sq=above.displacement.x.powi(2)+above.displacement.z.powi(2);
    if new_sq>old_sq+1e-9 {
        Movement {
            displacement:above.displacement,
            grounded:down.grounded || ordinary.grounded,
            blocked_x:(above.displacement.x-desired.x).abs()>1e-8,
            blocked_y:(above.displacement.y-desired.y).abs()>1e-8,
            blocked_z:(above.displacement.z-desired.z).abs()>1e-8,
        }
    } else {ordinary}
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Flat;
    impl CollisionWorld for Flat {
        fn boxes(&self,swept:Aabb)->Vec<Aabb> {
            let mut v=Vec::new();
            for x in (swept.min.x.floor() as i32)..(swept.max.x.ceil() as i32) {
                for z in (swept.min.z.floor() as i32)..(swept.max.z.ceil() as i32) {
                    // All blocks at y=-1 are solid cubes, floor top y=0.
                    if swept.min.y < 0.0 {v.push(Aabb::block(x,-1,z));}
                }
            }
            v
        }
    }
    #[test]
    fn gravity_collides_with_floor_and_jump_has_vertical_impulse() {
        let mut player=ClassicPlayer::new(0.5,3.0,0.5);
        for _ in 0..80 {player.tick(Controls::default(),&Flat).unwrap();}
        assert!((player.feet.y).abs()<0.001);
        assert!(player.on_ground);
        player.tick(Controls{jump:true,..Controls::default()},&Flat).unwrap();
        assert!(player.feet.y>0.30);
        assert!(!player.on_ground);
    }
    #[test]
    fn native_attack_has_no_cooldown_but_resets_sprint() {
        let mut player=ClassicPlayer::new(0.0,64.0,0.0);
        player.sprinting=true;
        assert_eq!(player.attack(5).unwrap(),[2,5,1]);
        assert!(!player.sprinting);
        assert_eq!(player.attack(5).unwrap(),[2,5,1]);
        assert!(player.attack(-1).is_err());
    }
    #[test]
    fn sword_block_uses_old_packet_not_modern_hand_enum() {
        let mut p=ClassicPlayer::new(0.0,64.0,0.0);
        let block=p.sword_block(None).unwrap();
        assert_eq!(block[0],8);
        assert_eq!(&block[1..9],&[255;8]);
        assert!(p.using_sword);
        assert_eq!(p.sword_release(),[7,5,0,0,0,0,0,0,0,0,0]);
        assert!(!p.using_sword);
    }
    #[test]
    fn movement_checks_nonfinite_inputs_and_server_corrections() {
        let mut p=ClassicPlayer::new(0.0,64.0,0.0);
        assert!(p.tick(Controls{forward:f64::NAN,..Controls::default()},&Flat).is_err());
        assert!(p.apply_server_velocity(f64::INFINITY,0.0,0.0).is_err());
        p.apply_server_velocity(1.0,0.5,0.0).unwrap();
        assert_eq!(p.velocity.x,1.0);
        p.apply_server_position(1.0,70.0,2.0,90.0,0.0).unwrap();
        assert_eq!(p.velocity,Vec3::default());
        assert_eq!(p.feet.y,70.0);
    }
}
