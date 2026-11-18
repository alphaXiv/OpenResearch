"""Render the actual bounded receipts collected from native orx, never input expectations."""
import json, os, platform, textwrap
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont
root=Path("evidence-opencode")
all={name:json.loads((root/(name+".json")).read_text()) for name in ("base","fixed","origin")}
for candidate,modes,title in [
 ("fixed",["timeout-metadata","timeout-asset"],"OpenCode setup: an intermittent vendor timeout"),
 ("origin",["refusal-metadata","refusal-asset","native"],"OpenCode setup: attribute a failed install or update")]:
    img=Image.new("RGB",(1500,240+245*len(modes)),"#f2f4f5");d=ImageDraw.Draw(img)
    font=lambda s:ImageFont.load_default(size=s)
    def put(pos,t,s=27,color="#122a3a"):d.text(pos,t,fill=color,font=font(s))
    put((40,24),title,36);put((40,78),"Native "+all[candidate][0]["platform"]+" | real orx setup socket + first-party event sent to loopback",21)
    for x,label,variant in [(40,"BEFORE","base"),(773,"AFTER",candidate)]:
        put((x,130),label+"  "+all[variant][0]["sha"][:12],25)
        for i,mode in enumerate(modes):
            r=next(row for row in all[variant] if row["mode"]==mode);w=r["wire"];y=177+245*i
            d.rounded_rectangle((x,y,x+688,y+224),10,fill="white",outline="#c5d3d9")
            put((x+18,y+12),mode,25)
            words=[f"setup WebSocket: {r['setupFrame']}",f"telemetry: {w['outcome']} / {w['stage']} / {w['reason'] or 'no reason'}","errorExcerpt: "+str(w['errorExcerpt']),"OpenCode --version: "+r['installedVersion'] if r['installedVersion'] else "npm TLS connections: "+str(r['npmTunnels'])]
            yy=y+52
            for word in words:
                for line in textwrap.wrap(word,57):put((x+18,yy),line,20);yy+=24
    img.save(root/(candidate+"-proof.png"))
