from fluid import *
import fluid
def pour(world,x,c,n,every=1,maxt=4000):
    placed=0
    for i in range(maxt):
        if placed<n and i%every==0 and world.m[0][x]=='.':
            world.m[0][x]=c; world.f[0][x]=FALL|(RIGHT if h(world.seed,world.tick,x,0,99)&1 else 0); world.vy[0][x]=0; placed+=1; world.step(); continue
        world.step()
        if placed==n and world.writes==0: return i
    return None
for seed in range(1,9):
    w=W(box(40,30),seed); t=pour(w,1,'w',400); hs=w.heights('w',1,41)
    print('water_levels seed',seed,'sleep@',t,'min/max',min(hs),max(hs),'count',w.count('w'))
    w=W(box(20,30),seed); t=pour(w,10,'w',107); hs=w.heights('w',1,21)
    print('partial_layer seed',seed,'sleep@',t,min(hs),max(hs))
    # gas under ceiling
    rows=['#'*22]+box(20,12); w=W(rows,seed)
    for i in range(27): w.m[12][1+i%20 if i<20 else i-19]='g' if w.m[12][1+i%20 if i<20 else i-19]=='.' else 'g'
    for i in range(7): w.m[11][3+i]='g'
    n0=w.count('g'); t=w.run_sleep(3000); print('gas seed',seed,'sleep@',t,'count',n0,w.count('g'), ''.join(w.m[1]), ''.join(w.m[2]))
    # mixed
    w=W(box(30,30),seed)
    for y in range(5,25):
        for x in range(3,27): w.m[y][x]='wos'[h(seed,0,x,y,7)%3]
    t=w.run_sleep(8000); print('mixed seed',seed,'sleep@',t)
# tunnel: pool left, 30-long tunnel at bottom row to the right chamber
rows=[]
for y in range(20):
    rows.append('#'+'.'*15+'#'*30+'.'*15+'#')
rows[19]='#'+'.'*60+'#'
rows.append('#'*62)
for press in (False,True):
    fluid.PRESS=press
    w=W(rows,1)
    for y in range(5,20):
        for x in range(1,16): w.m[y][x]='w'
    t=w.run_sleep(6000); print('tunnel press',press,'sleep@',t,'left',sum(w.heights('w',1,16)),'right',sum(w.heights('w',46,61)), 'hl',w.heights('w',1,16)[::5],'hr',w.heights('w',46,61)[::5])
fluid.PRESS=True
# breached wall
rows=box(50,20); w=W(rows,1)
for y in range(10,20):
    for x in range(1,21): w.m[y][x]='w'
    w.m[y][21]='#'
print('pool sleep',w.run_sleep(2000)); w.m[15][21]='.'
t=w.run_sleep(4000); print('breach sleep@',t,'left',w.heights('w',1,21)[::4],'right',w.heights('w',22,51)[::4])
# sand+water
w=W(box(40,30),1)
for i in range(300):
    if w.m[0][10]=='.': w.m[0][10]='d'; w.f[0][10]=FALL
    if w.m[0][30]=='.': w.m[0][30]='w'; w.f[0][30]=FALL
    w.step()
print('sand+water sleep',w.run_sleep(3000), w.count('d'), w.count('w'))
# slime vs water width
for c in 'ws':
    w=W(box(60,20),1)
    for y in range(10,20):
        for x in range(26,36): w.m[y][x]=c
    for i in range(100): w.step()
    hs=w.heights(c,1,61); print(c,'width@100',sum(1 for v in hs if v))
