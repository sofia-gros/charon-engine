"""仕様 02 (docs/spec/02-simulation.md) の移動規則の試作。

チャンクも dirty rect もない単純な 1 枚の格子で、粉体 (§7) と液体・気体 (§8) の規則だけを実装している。
仕様の規則が「必ず眠る」「平らになる」「トンネルを流れる」ことを確かめるために使った。
Rust の実装で挙動に迷ったら、この試作の `fluid()` と `powder()` を正とする。

実行: python3 tools/prototype/run.py
"""
import sys
M=(1<<64)-1
def h(seed,tick,x,y,salt):
    z=seed ^ (tick*0x9E3779B97F4A7C15 & M) ^ ((((x&0xffffffff)<<32)|(y&0xffffffff))*0xBF58476D1CE4E5B9 & M) ^ (salt*0x94D049BB133111EB & M)
    z=((z^(z>>30))*0xBF58476D1CE4E5B9)&M; z=((z^(z>>27))*0x94D049BB133111EB)&M
    return (z^(z>>31))&0xffffffff
E,S,P,L,G='E','S','P','L','G'
MAT={'.':dict(ph=E,den=0),'#':dict(ph=S,den=65535),
 'w':dict(ph=L,den=1000,disp=5,visc=0),'o':dict(ph=L,den=800,disp=3,visc=60),
 's':dict(ph=L,den=1100,disp=1,visc=240),'d':dict(ph=P,den=1600,fr=30),
 'g':dict(ph=G,den=1,disp=3,visc=0),'k':dict(ph=G,den=2,disp=2,visc=0)}
FALL,RIGHT,BOUNCED=1,2,4
SCAN=24
class W:
    def __init__(s,rows,seed=1):
        s.h=len(rows); s.w=len(rows[0]); s.seed=seed; s.tick=0
        s.m=[list(r) for r in rows]
        s.f=[[FALL|(RIGHT if h(seed,0,x,y,99)&1 else 0) for x in range(s.w)] for y in range(s.h)]
        s.vy=[[0]*s.w for _ in rows]; s.par=[[1]*s.w for _ in rows]
        s.writes=0
    def get(s,x,y):
        if 0<=x<s.w and 0<=y<s.h: return s.m[y][x]
        return '#'
    def swap(s,ax,ay,bx,by):
        for a in (s.m,s.f,s.vy): a[ay][ax],a[by][bx]=a[by][bx],a[ay][ax]
        s.par[ay][ax]=s.par[by][bx]=s.tick&1; s.writes+=1
    def candisp(s,mv,t):
        tm=MAT[t]; mm=MAT[mv]
        if tm['ph']==E: return True
        if tm['ph'] in (S,P): return False
        if tm['ph']==L: return mm['ph'] in (P,L) and mm['den']>tm['den']
        if tm['ph']==G: return mm['den']>tm['den'] if mm['ph']==G else True
    def canrise(s,mv,t):
        tm=MAT[t]
        if tm['ph']==E: return True
        return tm['ph']==G and MAT[mv]['den']<tm['den']
    def step(s):
        s.writes=0; t=s.tick; cp=t&1
        for y in range(s.h-1,-1,-1):
            xs=range(s.w) if h(s.seed,t,0,y,1)&1==0 else range(s.w-1,-1,-1)
            for x in xs:
                c=s.m[y][x]; ph=MAT[c]['ph']
                if ph in (E,S) or s.par[y][x]==cp: continue
                if ph==P: s.powder(x,y,c)
                else: s.fluid(x,y,c)
        # set parity for all
        for y in range(s.h):
            for x in range(s.w): s.par[y][x]=cp
        s.tick+=1
    def steps(s,x,y,g=4,mx=96):
        vy=min(s.vy[y][x]+g,mx); n=vy//16
        if (h(s.seed,s.tick,x,y,2)&15)<vy%16: n+=1
        return max(n,1),vy
    def powder(s,x,y,c):
        t=s.tick
        if s.candisp(c,s.get(x,y+1)):
            if MAT[s.get(x,y+1)]['ph']==L and (h(s.seed,t,x,y,3)&255)<MAT[s.get(x,y+1)]['visc']//2+64:
                s.writes+=1; return
            n,vy=s.steps(x,y); mv=0; ox,oy=x,y
            while mv<n and s.candisp(c,s.get(x,y+1)): s.swap(x,y,x,y+1); y+=1; mv+=1
            s.f[y][x]|=FALL; s.vy[y][x]=vy if mv==n else 0; s.wake(ox,oy); return
        if s.f[y][x]&FALL and not (h(s.seed,t,x,y,8)&255)<MAT[c]['fr']:
            d=1 if h(s.seed,t,x,y,4)&1 else -1
            for dx in (d,-d):
                if s.candisp(c,s.get(x+dx,y+1)) and s.candisp(c,s.get(x+dx,y)):
                    s.swap(x,y,x+dx,y+1); s.vy[y+1][x+dx]=0; s.wake(x,y); return
            s.f[y][x]&=~FALL; s.vy[y][x]=0; s.writes+=1
    def wake(s,x,y):
        for nx,ny in ((x-1,y),(x+1,y),(x,y-1)):
            n=s.get(nx,ny)
            if MAT[n]['ph']==P and 0<=nx<s.w and 0<=ny<s.h and not s.f[ny][nx]&FALL:
                if (h(s.seed,s.tick,nx,ny,5)&255)<255-MAT[n]['fr']: s.f[ny][nx]|=FALL; s.writes+=1
    def fluid(s,x,y,c):
        t=s.tick; m=MAT[c]; up=m['ph']==G; dy=-1 if up else 1
        pas=(lambda q: s.canrise(c,q)) if up else (lambda q: s.candisp(c,q))
        skip_p=128 if up else m['visc']
        skip=lambda: (h(s.seed,t,x,y,6)&255)<skip_p
        d=1 if s.f[y][x]&RIGHT else -1
        if pas(s.get(x,y+dy)):
            if skip(): s.writes+=1; return
            n,vy=(1,0) if up else s.steps(x,y); mv=0
            while mv<n and pas(s.get(x,y+dy)): s.swap(x,y,x,y+dy); y+=dy; mv+=1
            s.f[y][x]=(s.f[y][x]|FALL)&~BOUNCED; s.vy[y][x]=vy if mv==n else 0; return
        for dx in (d,-d):
            if pas(s.get(x+dx,y+dy)) and pas(s.get(x+dx,y)):
                if skip(): s.writes+=1; return
                s.swap(x,y,x+dx,y+dy); f=(s.f[y+dy][x+dx]|FALL)&~BOUNCED&~RIGHT
                s.f[y+dy][x+dx]=f|(RIGHT if dx>0 else 0); s.vy[y+dy][x+dx]=0; return
        s.vy[y][x]=0
        opn={}
        for dr in (d,-d):
            opn[dr]=0; goal=None
            for i in range(1,SCAN+1):
                if not pas(s.get(x+dr*i,y)): break
                opn[dr]=i
                if pas(s.get(x+dr*i,y+dy)): goal=i; break
            if goal:
                if skip(): s.writes+=1; return
                n=min(goal,max(m['disp'],1)); s.swap(x,y,x+dr*n,y)
                s.f[y][x+dr*n]=(s.f[y][x+dr*n]&~RIGHT)|(RIGHT if dr>0 else 0); return
        b=s.get(x,y-dy); bm=MAT[b]
        pressed= bm['ph']==m['ph'] and ((bm['den']<=m['den']) if up else (bm['den']>=m['den']))
        if PRESS and pressed:
            for dr in (d,-d):
                if opn[dr]>0:
                    if skip(): s.writes+=1; return
                    n=min(opn[dr],max(m['disp'],1)); s.swap(x,y,x+dr*n,y)
                    s.f[y][x+dr*n]=((s.f[y][x+dr*n]&~RIGHT&~BOUNCED)|FALL)|(RIGHT if dr>0 else 0); return
            return
        if not s.f[y][x]&FALL: return
        if opn[d]>0:
            if skip(): s.writes+=1; return
            n=min(opn[d],max(m['disp'],1)); s.swap(x,y,x+d*n,y); return
        if MAT[s.get(x+d,y)]['ph']==m['ph']: return
        if not s.f[y][x]&BOUNCED:
            s.f[y][x]=(s.f[y][x]|BOUNCED)^RIGHT; s.writes+=1; return
        s.f[y][x]&=~(FALL|BOUNCED); s.writes+=1
    def count(s,c): return sum(r.count(c) for r in s.m)
    def show(s): print('\n'.join(''.join(r) for r in s.m))
    def run_sleep(s,maxt):
        for i in range(maxt):
            s.step()
            if s.writes==0: return i
        return None
    def heights(s,c,x0,x1):
        return [sum(1 for y in range(s.h) if s.m[y][x]==c) for x in range(x0,x1)]
PRESS=True
def box(w,h): return ['#'+'.'*w+'#' for _ in range(h)]+['#'*(w+2)]
