import{p as P}from"./chunk-JWPE2WC7-BDkB-9IX.js";import{_ as f,P as B,S as z,d as E,l as $,b as F,a as A,v as W,x as _,g as N,s as L,N as M,Q as Y,B as I}from"./mermaid.core-BhMlSGJ6.js";import{p as O}from"./cynefin-EF2NZ3EQ-BXMpGjnh.js";import"./index-B4N79X3K.js";import"./modulepreload-polyfill-B5Qt9EMX.js";import"./_commonjsHelpers-CE1G-McA.js";import"./marked.esm-DoTFXBQ3.js";var j=Y.packet,b,C=(b=class{constructor(){this.packet=[],this.setAccTitle=F,this.getAccTitle=A,this.setDiagramTitle=W,this.getDiagramTitle=_,this.getAccDescription=N,this.setAccDescription=L}getConfig(){const t=B({...j,...M().packet});return t.showBits&&(t.paddingY+=10),t}getPacket(){return this.packet}pushWord(t){t.length>0&&this.packet.push(t)}clear(){I(),this.packet=[]}},f(b,"PacketDB"),b),G=1e4,H=f((e,t)=>{P(e,t);let r=-1,o=[],i=1;const{bitsPerRow:l}=t.getConfig();for(let{start:a,end:s,bits:d,label:g}of e.blocks){if(a!==void 0&&s!==void 0&&s<a)throw new Error(`Packet block ${a} - ${s} is invalid. End must be greater than start.`);if(a??=r+1,a!==r+1)throw new Error(`Packet block ${a} - ${s??a} is not contiguous. It should start from ${r+1}.`);if(d===0)throw new Error(`Packet block ${a} is invalid. Cannot have a zero bit field.`);for(s??=a+(d??1)-1,d??=s-a+1,r=s,$.debug(`Packet block ${a} - ${r} with label ${g}`);o.length<=l+1&&t.getPacket().length<G;){const[c,p]=K({start:a,end:s,bits:d,label:g},i,l);if(o.push(c),c.end+1===i*l&&(t.pushWord(o),o=[],i++),!p)break;({start:a,end:s,bits:d,label:g}=p)}}t.pushWord(o)},"populate"),K=f((e,t,r)=>{if(e.start===void 0)throw new Error("start should have been set during first phase");if(e.end===void 0)throw new Error("end should have been set during first phase");if(e.start>e.end)throw new Error(`Block start ${e.start} is greater than block end ${e.end}.`);if(e.end+1<=t*r)return[e,void 0];const o=t*r-1,i=t*r;return[{start:e.start,end:o,label:e.label,bits:o-e.start},{start:i,end:e.end,label:e.label,bits:e.end-i}]},"getNextFittingBlock"),S={parser:{yy:void 0},parse:f(async e=>{const t=await O("packet",e),r=S.parser?.yy;if(!(r instanceof C))throw new Error("parser.parser?.yy was not a PacketDB. This is due to a bug within Mermaid, please report this issue at https://github.com/mermaid-js/mermaid/issues.");$.debug(t),H(t,r)},"parse")},Q=f((e,t,r,o)=>{const i=o.db,l=i.getConfig(),{rowHeight:a,paddingY:s,bitWidth:d,bitsPerRow:g}=l,c=i.getPacket(),p=i.getDiagramTitle(),m=a+s,n=m*(c.length+1)-(p?0:a),h=d*g+2,k=z(t);k.attr("viewBox",`0 0 ${h} ${n}`),E(k,n,h,l.useMaxWidth);for(const[x,u]of c.entries())U(k,u,x,l);k.append("text").text(p).attr("x",h/2).attr("y",n-m/2).attr("dominant-baseline","middle").attr("text-anchor","middle").attr("class","packetTitle")},"draw"),U=f((e,t,r,{rowHeight:o,paddingX:i,paddingY:l,bitWidth:a,bitsPerRow:s,showBits:d,bitOrder:g})=>{const c=e.append("g"),p=r*(o+l)+l,m=g==="descending";for(const n of t){const h=n.end-n.start+1,k=n.start%s,u=(m?s-k-h:k)*a+1,w=h*a-i;if(c.append("rect").attr("x",u).attr("y",p).attr("width",w).attr("height",o).attr("class","packetBlock"),c.append("text").attr("x",u+w/2).attr("y",p+o/2).attr("class","packetLabel").attr("dominant-baseline","middle").attr("text-anchor","middle").text(n.label),!d)continue;const[D,T]=m?[n.end,n.start]:[n.start,n.end],v=h===1,y=p-2;c.append("text").attr("x",u+(v?w/2:0)).attr("y",y).attr("class","packetByte start").attr("dominant-baseline","auto").attr("text-anchor",v?"middle":"start").text(D),v||c.append("text").attr("x",u+w).attr("y",y).attr("class","packetByte end").attr("dominant-baseline","auto").attr("text-anchor","end").text(T)}},"drawWord"),X={draw:Q},q={byteFontSize:"10px",startByteColor:"black",endByteColor:"black",labelColor:"black",labelFontSize:"12px",titleColor:"black",titleFontSize:"14px",blockStrokeColor:"black",blockStrokeWidth:"1",blockFillColor:"#efefef"},J=f(({packet:e}={})=>{const t=B(q,e);return`
	.packetByte {
		font-size: ${t.byteFontSize};
	}
	.packetByte.start {
		fill: ${t.startByteColor};
	}
	.packetByte.end {
		fill: ${t.endByteColor};
	}
	.packetLabel {
		fill: ${t.labelColor};
		font-size: ${t.labelFontSize};
	}
	.packetTitle {
		fill: ${t.titleColor};
		font-size: ${t.titleFontSize};
	}
	.packetBlock {
		stroke: ${t.blockStrokeColor};
		stroke-width: ${t.blockStrokeWidth};
		fill: ${t.blockFillColor};
	}
	`},"styles"),ot={parser:S,get db(){return new C},renderer:X,styles:J};export{ot as diagram};
