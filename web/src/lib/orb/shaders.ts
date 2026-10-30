/** GLSL for the Jarvis orb. 3D simplex noise by Ian McEwan / Stefan Gustavson (MIT). */

export const NOISE = /* glsl */ `
vec3 mod289(vec3 x){return x-floor(x*(1.0/289.0))*289.0;}
vec4 mod289(vec4 x){return x-floor(x*(1.0/289.0))*289.0;}
vec4 permute(vec4 x){return mod289(((x*34.0)+10.0)*x);}
vec4 taylorInvSqrt(vec4 r){return 1.79284291400159-0.85373472095314*r;}
float snoise(vec3 v){
  const vec2 C=vec2(1.0/6.0,1.0/3.0);
  const vec4 D=vec4(0.0,0.5,1.0,2.0);
  vec3 i=floor(v+dot(v,C.yyy));
  vec3 x0=v-i+dot(i,C.xxx);
  vec3 g=step(x0.yzx,x0.xyz);
  vec3 l=1.0-g;
  vec3 i1=min(g.xyz,l.zxy);
  vec3 i2=max(g.xyz,l.zxy);
  vec3 x1=x0-i1+C.xxx;
  vec3 x2=x0-i2+C.yyy;
  vec3 x3=x0-D.yyy;
  i=mod289(i);
  vec4 p=permute(permute(permute(
    i.z+vec4(0.0,i1.z,i2.z,1.0))
    +i.y+vec4(0.0,i1.y,i2.y,1.0))
    +i.x+vec4(0.0,i1.x,i2.x,1.0));
  float n_=0.142857142857;
  vec3 ns=n_*D.wyz-D.xzx;
  vec4 j=p-49.0*floor(p*ns.z*ns.z);
  vec4 x_=floor(j*ns.z);
  vec4 y_=floor(j-7.0*x_);
  vec4 x=x_*ns.x+ns.yyyy;
  vec4 y=y_*ns.x+ns.yyyy;
  vec4 h=1.0-abs(x)-abs(y);
  vec4 b0=vec4(x.xy,y.xy);
  vec4 b1=vec4(x.zw,y.zw);
  vec4 s0=floor(b0)*2.0+1.0;
  vec4 s1=floor(b1)*2.0+1.0;
  vec4 sh=-step(h,vec4(0.0));
  vec4 a0=b0.xzyw+s0.xzyw*sh.xxyy;
  vec4 a1=b1.xzyw+s1.xzyw*sh.zzww;
  vec3 p0=vec3(a0.xy,h.x);
  vec3 p1=vec3(a0.zw,h.y);
  vec3 p2=vec3(a1.xy,h.z);
  vec3 p3=vec3(a1.zw,h.w);
  vec4 norm=taylorInvSqrt(vec4(dot(p0,p0),dot(p1,p1),dot(p2,p2),dot(p3,p3)));
  p0*=norm.x;p1*=norm.y;p2*=norm.z;p3*=norm.w;
  vec4 m=max(0.5-vec4(dot(x0,x0),dot(x1,x1),dot(x2,x2),dot(x3,x3)),0.0);
  m=m*m;
  return 105.0*dot(m*m,vec4(dot(p0,x0),dot(p1,x1),dot(p2,x2),dot(p3,x3)));
}
`;

/** The energy sphere: noise-displaced surface, flowing colour field, fresnel rim, listening ripples. */
export const CORE_VERT = /* glsl */ `
${NOISE}
uniform float uTime;
uniform float uSwirl;
uniform float uDisp;
uniform float uAmp;
varying vec3 vNormal;
varying vec3 vView;
varying vec3 vPos;
void main(){
  vec3 p=position;
  float n=snoise(p*1.5+vec3(0.0,uTime*0.35*(0.4+uSwirl),uTime*0.2));
  float d=n*uDisp*(1.0+uAmp*2.2)+uAmp*0.05;
  vec3 displaced=p+normal*d;
  vec4 mv=modelViewMatrix*vec4(displaced,1.0);
  vNormal=normalize(normalMatrix*normal);
  vView=normalize(-mv.xyz);
  vPos=p;
  gl_Position=projectionMatrix*mv;
}
`;

export const CORE_FRAG = /* glsl */ `
${NOISE}
uniform float uTime;
uniform float uSwirl;
uniform float uEnergy;
uniform float uRim;
uniform float uRipple;
uniform float uFlash;
uniform float uAmp;
uniform vec3 uColorA;
uniform vec3 uColorB;
varying vec3 vNormal;
varying vec3 vView;
varying vec3 vPos;

mat2 rot(float a){float c=cos(a),s=sin(a);return mat2(c,-s,s,c);}

void main(){
  vec3 N=normalize(vNormal);
  vec3 V=normalize(vView);
  float fres=pow(1.0-clamp(dot(N,V),0.0,1.0),2.4);

  // Flowing colour field: rotate the sample point, then two octaves of noise.
  vec3 q=vPos;
  q.xz*=rot(uTime*0.25*uSwirl);
  float f1=snoise(q*2.1+vec3(0.0,uTime*0.18*uSwirl,0.0));
  float f2=snoise(q*4.3-vec3(uTime*0.25*uSwirl,0.0,0.0));
  float field=0.5+0.5*(f1*0.7+f2*0.3);

  vec3 body=mix(uColorB,uColorA,smoothstep(0.25,0.9,field));
  // Thin bright veins where the field crosses a band.
  float vein=smoothstep(0.05,0.0,abs(field-0.6))*0.7;

  // Listening ripples travel up the sphere.
  float ripple=uRipple*smoothstep(0.55,1.0,sin(vPos.y*11.0-uTime*5.0))*(0.35+0.65*fres);

  vec3 col=body*(0.2+0.42*field)*uEnergy;
  col+=uColorA*vein*0.5*uEnergy;
  col+=uColorA*fres*uRim*1.7;
  col+=vec3(0.85,0.97,1.0)*pow(fres,4.0)*uRim*1.0;   // hot white edge
  col+=uColorA*pow(1.0-fres,3.0)*0.1*uEnergy;        // faint inner glow
  col+=uColorA*ripple*0.9;
  col+=uColorA*uAmp*0.35*(0.4+fres);
  col=mix(col,vec3(1.0,0.2,0.26)*(0.5+fres*1.4),uFlash*0.78);

  float alpha=clamp(0.42+fres*1.0+vein*0.25,0.0,1.0);
  gl_FragColor=vec4(col,alpha);
}
`;

/** Soft atmosphere hugging the sphere (rendered on the back faces of a larger sphere). */
export const SHELL_VERT = /* glsl */ `
varying vec3 vNormal;
varying vec3 vView;
void main(){
  vec4 mv=modelViewMatrix*vec4(position,1.0);
  vNormal=normalize(normalMatrix*normal);
  vView=normalize(-mv.xyz);
  gl_Position=projectionMatrix*mv;
}
`;

export const SHELL_FRAG = /* glsl */ `
uniform vec3 uColor;
uniform float uIntensity;
varying vec3 vNormal;
varying vec3 vView;
void main(){
  // Back faces: |dot| is 0 at the shell's silhouette and grows toward the core's edge.
  float d=abs(dot(normalize(vNormal),normalize(vView)));
  float a=pow(d,1.7)*uIntensity;
  gl_FragColor=vec4(uColor,a);
}
`;

/** Camera-facing radial halo: cheap stand-in for a bloom pass. */
export const HALO_VERT = /* glsl */ `
varying vec2 vUv;
void main(){vUv=uv;gl_Position=projectionMatrix*modelViewMatrix*vec4(position,1.0);}
`;

export const HALO_FRAG = /* glsl */ `
uniform vec3 uColor;
uniform float uIntensity;
uniform float uTime;
varying vec2 vUv;
void main(){
  vec2 p=vUv-0.5;
  float r=length(p)*2.0;
  float core=exp(-pow(r*1.55,2.0)*1.6);
  float tail=exp(-r*3.6);
  float ring=smoothstep(0.035,0.0,abs(r-0.62-0.01*sin(uTime*0.8)))*0.25;
  float fade=smoothstep(1.0,0.55,r);
  float a=(core*0.55+tail*0.55+ring)*uIntensity*fade;
  gl_FragColor=vec4(uColor,a);
}
`;

/** Orbiting motes. */
export const MOTE_VERT = /* glsl */ `
uniform float uTime;
uniform float uPx;
attribute float aSize;
attribute float aPhase;
varying float vTw;
void main(){
  vec4 mv=modelViewMatrix*vec4(position,1.0);
  vTw=0.55+0.45*sin(uTime*1.3+aPhase*6.2831);
  gl_PointSize=aSize*uPx*(4.0/-mv.z);
  gl_Position=projectionMatrix*mv;
}
`;

export const MOTE_FRAG = /* glsl */ `
uniform vec3 uColor;
uniform float uIntensity;
varying float vTw;
void main(){
  float d=length(gl_PointCoord-0.5)*2.0;
  float a=smoothstep(1.0,0.0,d);
  a*=a*vTw*uIntensity;
  gl_FragColor=vec4(uColor,a);
}
`;
