import{p as rt}from"./chunk-JWPE2WC7-9SFBizQI.js";import{a9 as T,ac as G,ba as nt,g as it,s as st,a as ot,b as lt,q as ct,p as ut,_ as d,l as B,c as gt,M as dt,P as pt,U as ht,d as ft,z as mt,N as vt}from"./mermaid.core-UH_fzj7S.js";import{p as xt}from"./cynefin-EF2NZ3EQ-DZE6xqrs.js";import"./index-DqRDe6u-.js";import{d as J}from"./arc-DjQaW6m5.js";import{o as St}from"./ordinal-Cboi1Yqb.js";import"./init-Gi6I4Gst.js";function yt(t,n){return n<t?-1:n>t?1:n>=t?0:NaN}function wt(t){return t}function At(){var t=wt,n=yt,y=null,b=T(0),l=T(G),p=T(0);function i(e){var r,o=(e=nt(e)).length,h,w,$=0,f=new Array(o),s=new Array(o),D=+b.apply(this,arguments),M=Math.min(G,Math.max(-G,l.apply(this,arguments)-D)),k,L=Math.min(Math.abs(M)/o,p.apply(this,arguments)),u=L*(M<0?-1:1),A;for(r=0;r<o;++r)(A=s[f[r]=r]=+t(e[r],r,e))>0&&($+=A);for(n!=null?f.sort(function(E,m){return n(s[E],s[m])}):y!=null&&f.sort(function(E,m){return y(e[E],e[m])}),r=0,w=$?(M-o*u)/$:0;r<o;++r,D=k)h=f[r],A=s[h],k=D+(A>0?A*w:0)+u,s[h]={data:e[h],index:r,value:A,startAngle:D,endAngle:k,padAngle:L};return s}return i.value=function(e){return arguments.length?(t=typeof e=="function"?e:T(+e),i):t},i.sortValues=function(e){return arguments.length?(n=e,y=null,i):n},i.sort=function(e){return arguments.length?(y=e,n=null,i):y},i.startAngle=function(e){return arguments.length?(b=typeof e=="function"?e:T(+e),i):b},i.endAngle=function(e){return arguments.length?(l=typeof e=="function"?e:T(+e),i):l},i.padAngle=function(e){return arguments.length?(p=typeof e=="function"?e:T(+e),i):p},i}var Ct=vt.pie,I={sections:new Map,showData:!1},W=I.sections,U=I.showData,$t=structuredClone(Ct),Dt=d(()=>structuredClone($t),"getConfig"),Tt=d(()=>{W=new Map,U=I.showData,mt()},"clear"),bt=d(({label:t,value:n})=>{if(n<0)throw new Error(`"${t}" has invalid value: ${n}. Negative values are not allowed in pie charts. All slice values must be >= 0.`);W.has(t)||(W.set(t,n),B.debug(`added new section: ${t}, with value: ${n}`))},"addSection"),kt=d(()=>W,"getSections"),zt=d(t=>{U=t},"setShowData"),Mt=d(()=>U,"getShowData"),K={getConfig:Dt,clear:Tt,setDiagramTitle:ut,getDiagramTitle:ct,setAccTitle:lt,getAccTitle:ot,setAccDescription:st,getAccDescription:it,addSection:bt,getSections:kt,setShowData:zt,getShowData:Mt},Et=d((t,n)=>{rt(t,n),n.setShowData(t.showData),t.sections.map(n.addSection)},"populateDb"),Rt={parse:d(async t=>{const n=await xt("pie",t);B.debug(n),Et(n,K)},"parse")},Lt=d(t=>`
  .pieCircle{
    stroke: ${t.pieStrokeColor};
    stroke-width : ${t.pieStrokeWidth};
    opacity : ${t.pieOpacity};
  }
  .pieCircle.highlighted{
    scale: 1.05;
    opacity: 1;
  }
  .pieCircle.highlightedOnHover:hover{
    transition-duration: 250ms;
    scale: 1.05;
    opacity: 1;
  }
  .pieOuterCircle{
    stroke: ${t.pieOuterStrokeColor};
    stroke-width: ${t.pieOuterStrokeWidth};
    fill: none;
  }
  .pieTitleText {
    text-anchor: middle;
    font-size: ${t.pieTitleTextSize};
    fill: ${t.pieTitleTextColor};
    font-family: ${t.fontFamily};
  }
  .slice {
    font-family: ${t.fontFamily};
    fill: ${t.pieSectionTextColor};
    font-size:${t.pieSectionTextSize};
    // fill: white;
  }
  .legend text {
    fill: ${t.pieLegendTextColor};
    font-family: ${t.fontFamily};
    font-size: ${t.pieLegendTextSize};
  }
`,"getStyles"),Nt=Lt,Pt=d(t=>{const n=[...t.values()].reduce((l,p)=>l+p,0),y=[...t.entries()].map(([l,p])=>({label:l,value:p})).filter(l=>l.value/n*100>=1);return At().value(l=>l.value).sort(null)(y)},"createPieArcs"),Wt=d((t,n,y,b)=>{var Z;B.debug(`rendering pie chart
`+t);const l=b.db,p=gt(),i=dt(l.getConfig(),p.pie),e=40,r=18,o=4,h=450,w=h,$=pt(n),f=$.append("g");f.attr("transform","translate("+w/2+","+h/2+")");const{themeVariables:s}=p;let[D]=ht(s.pieOuterStrokeWidth);D??(D=2);const M=i.legendPosition,k=i.textPosition,L=i.donutHole>0&&i.donutHole<=.9?i.donutHole:0,u=Math.min(w,h)/2-e,A=J().innerRadius(L*u).outerRadius(u),E=J().innerRadius(u*k).outerRadius(u*k),m=f.append("g");m.append("circle").attr("cx",0).attr("cy",0).attr("r",u+D/2).attr("class","pieOuterCircle");const N=l.getSections(),Q=Pt(N),Y=[s.pie1,s.pie2,s.pie3,s.pie4,s.pie5,s.pie6,s.pie7,s.pie8,s.pie9,s.pie10,s.pie11,s.pie12];let _=0;N.forEach(a=>{_+=a});const V=Q.filter(a=>(a.data.value/_*100).toFixed(0)!=="0"),F=St(Y).domain([...N.keys()]);m.selectAll("mySlices").data(V).enter().append("path").attr("d",A).attr("fill",a=>F(a.data.label)).attr("class",a=>{let c="pieCircle";return i.highlightSlice==="hover"?c+=" highlightedOnHover":i.highlightSlice===a.data.label&&(c+=" highlighted"),c}),m.selectAll("mySlices").data(V).enter().append("text").text(a=>(a.data.value/_*100).toFixed(0)+"%").attr("transform",a=>"translate("+E.centroid(a)+")").style("text-anchor","middle").attr("class","slice");const tt=f.append("text").text(l.getDiagramTitle()).attr("x",0).attr("y",-400/2).attr("class","pieTitleText"),R=[...N.entries()].map(([a,c])=>({label:a,value:c})),C=f.selectAll(".legend").data(R).enter().append("g").attr("class","legend");C.append("rect").attr("width",r).attr("height",r).style("fill",a=>F(a.label)).style("stroke",a=>F(a.label)),C.append("text").attr("x",r+o).attr("y",r-o).text(a=>l.getShowData()?`${a.label} [${a.value}]`:a.label);const z=Math.max(...C.selectAll("text").nodes().map(a=>(a==null?void 0:a.getBoundingClientRect().width)??0));let P=h,H=w+e;const g=r+o,O=R.length*g;switch(M){case"center":C.attr("transform",(a,c)=>{const v=g*R.length/2,x=-z/2-(r+o),S=c*g-v;return"translate("+x+","+S+")"});break;case"top":P+=O,C.attr("transform",(a,c)=>{const v=u,x=-z/2-(r+o),S=c*g-v;return`translate(${x}, ${S})`}),m.attr("transform",()=>`translate(0, ${O+g})`);break;case"bottom":P+=O,C.attr("transform",(a,c)=>{const v=-u-g,x=-z/2-(r+o),S=c*g-v;return"translate("+x+","+S+")"});break;case"left":H+=r+o+z,C.attr("transform",(a,c)=>{const v=g*R.length/2,x=-u-(r+o),S=c*g-v;return"translate("+x+","+S+")"}),m.attr("transform",()=>`translate(${z+r+o}, 0)`);break;case"right":default:H+=r+o+z,C.attr("transform",(a,c)=>{const v=g*R.length/2,x=12*r,S=c*g-v;return"translate("+x+","+S+")"});break}const j=((Z=tt.node())==null?void 0:Z.getBoundingClientRect().width)??0,et=w/2-j/2,at=w/2+j/2,q=Math.min(0,et),X=Math.max(H,at)-q;$.attr("viewBox",`${q} 0 ${X} ${P}`),ft($,P,X,i.useMaxWidth)},"draw"),_t={draw:Wt},jt={parser:Rt,db:K,renderer:_t,styles:Nt};export{jt as diagram};
