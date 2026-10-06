import{g as ie}from"./chunk-XXDRQBXY-D8Qj6glK.js";import{s as re}from"./chunk-KQW6MTUR-DHkqmCTN.js";import{_ as p,l as b,c as B,o as ae,r as ne,u as oe,a as le,b as ce,g as he,s as de,p as ue,q as fe,Y as pe,k as K,z as ge,y as Se,B as ye,C as Te,D as me,H as Ee}from"./mermaid.core-CSpv9VG6.js";import{c as _e}from"./chunk-7TKQ45FW-DsoA74vc.js";import{s as vt}from"./index-CjePeJgV.js";var At=(function(){var t=p(function(j,c,f,o){for(f=f||{},o=j.length;o--;f[j[o]]=c);return f},"o"),e=[1,2],s=[1,3],a=[1,4],i=[2,4],n=[1,9],l=[1,11],d=[1,16],h=[1,17],y=[1,18],T=[1,19],_=[1,33],I=[1,20],x=[1,21],L=[1,22],N=[1,23],$=[1,24],D=[1,26],g=[1,27],v=[1,28],F=[1,29],Y=[1,30],C=[1,31],G=[1,32],P=[1,35],ot=[1,36],lt=[1,37],ct=[1,38],J=[1,34],S=[1,4,5,16,17,19,21,22,24,25,26,27,28,29,33,35,37,38,41,45,48,51,52,53,54,57],ht=[1,4,5,14,15,16,17,19,21,22,24,25,26,27,28,29,33,35,37,38,39,40,41,45,48,51,52,53,54,57],It=[4,5,16,17,19,21,22,24,25,26,27,28,29,33,35,37,38,41,45,48,51,52,53,54,57],Tt={trace:p(function(){},"trace"),yy:{},symbols_:{error:2,start:3,SPACE:4,NL:5,SD:6,document:7,line:8,statement:9,classDefStatement:10,styleStatement:11,cssClassStatement:12,idStatement:13,DESCR:14,"-->":15,HIDE_EMPTY:16,scale:17,WIDTH:18,COMPOSIT_STATE:19,STRUCT_START:20,STRUCT_STOP:21,STATE_DESCR:22,AS:23,ID:24,FORK:25,JOIN:26,CHOICE:27,CONCURRENT:28,note:29,notePosition:30,NOTE_TEXT:31,direction:32,acc_title:33,acc_title_value:34,acc_descr:35,acc_descr_value:36,acc_descr_multiline_value:37,CLICK:38,STRING:39,HREF:40,classDef:41,CLASSDEF_ID:42,CLASSDEF_STYLEOPTS:43,DEFAULT:44,style:45,STYLE_IDS:46,STYLEDEF_STYLEOPTS:47,class:48,CLASSENTITY_IDS:49,STYLECLASS:50,direction_tb:51,direction_bt:52,direction_rl:53,direction_lr:54,eol:55,";":56,EDGE_STATE:57,STYLE_SEPARATOR:58,left_of:59,right_of:60,$accept:0,$end:1},terminals_:{2:"error",4:"SPACE",5:"NL",6:"SD",14:"DESCR",15:"-->",16:"HIDE_EMPTY",17:"scale",18:"WIDTH",19:"COMPOSIT_STATE",20:"STRUCT_START",21:"STRUCT_STOP",22:"STATE_DESCR",23:"AS",24:"ID",25:"FORK",26:"JOIN",27:"CHOICE",28:"CONCURRENT",29:"note",31:"NOTE_TEXT",33:"acc_title",34:"acc_title_value",35:"acc_descr",36:"acc_descr_value",37:"acc_descr_multiline_value",38:"CLICK",39:"STRING",40:"HREF",41:"classDef",42:"CLASSDEF_ID",43:"CLASSDEF_STYLEOPTS",44:"DEFAULT",45:"style",46:"STYLE_IDS",47:"STYLEDEF_STYLEOPTS",48:"class",49:"CLASSENTITY_IDS",50:"STYLECLASS",51:"direction_tb",52:"direction_bt",53:"direction_rl",54:"direction_lr",56:";",57:"EDGE_STATE",58:"STYLE_SEPARATOR",59:"left_of",60:"right_of"},productions_:[0,[3,2],[3,2],[3,2],[7,0],[7,2],[8,2],[8,1],[8,1],[9,1],[9,1],[9,1],[9,1],[9,2],[9,3],[9,4],[9,1],[9,2],[9,1],[9,4],[9,3],[9,6],[9,1],[9,1],[9,1],[9,1],[9,4],[9,4],[9,1],[9,2],[9,2],[9,1],[9,5],[9,5],[10,3],[10,3],[11,3],[12,3],[32,1],[32,1],[32,1],[32,1],[55,1],[55,1],[13,1],[13,1],[13,3],[13,3],[30,1],[30,1]],performAction:p(function(c,f,o,m,E,r,V){var u=r.length-1;switch(E){case 3:return m.setRootDoc(r[u]),r[u];case 4:this.$=[];break;case 5:r[u]!="nl"&&(r[u-1].push(r[u]),this.$=r[u-1]);break;case 6:case 7:this.$=r[u];break;case 8:this.$="nl";break;case 12:this.$=r[u];break;case 13:const et=r[u-1];et.description=m.trimColon(r[u]),this.$=et;break;case 14:this.$={stmt:"relation",state1:r[u-2],state2:r[u]};break;case 15:const mt=m.trimColon(r[u]);this.$={stmt:"relation",state1:r[u-3],state2:r[u-1],description:mt};break;case 19:this.$={stmt:"state",id:r[u-3],type:"default",description:"",doc:r[u-1]};break;case 20:var M=r[u],q=r[u-2].trim();if(r[u].match(":")){var ut=r[u].split(":");M=ut[0],q=[q,ut[1]]}this.$={stmt:"state",id:M,type:"default",description:q};break;case 21:this.$={stmt:"state",id:r[u-3],type:"default",description:r[u-5],doc:r[u-1]};break;case 22:this.$={stmt:"state",id:r[u],type:"fork"};break;case 23:this.$={stmt:"state",id:r[u],type:"join"};break;case 24:this.$={stmt:"state",id:r[u],type:"choice"};break;case 25:this.$={stmt:"state",id:m.getDividerId(),type:"divider"};break;case 26:this.$={stmt:"state",id:r[u-1].trim(),note:{position:r[u-2].trim(),text:r[u].trim()}};break;case 29:this.$=r[u].trim(),m.setAccTitle(this.$);break;case 30:case 31:this.$=r[u].trim(),m.setAccDescription(this.$);break;case 32:this.$={stmt:"click",id:r[u-3],url:r[u-2],tooltip:r[u-1]};break;case 33:this.$={stmt:"click",id:r[u-3],url:r[u-1],tooltip:""};break;case 34:case 35:this.$={stmt:"classDef",id:r[u-1].trim(),classes:r[u].trim()};break;case 36:this.$={stmt:"style",id:r[u-1].trim(),styleClass:r[u].trim()};break;case 37:this.$={stmt:"applyClass",id:r[u-1].trim(),styleClass:r[u].trim()};break;case 38:m.setDirection("TB"),this.$={stmt:"dir",value:"TB"};break;case 39:m.setDirection("BT"),this.$={stmt:"dir",value:"BT"};break;case 40:m.setDirection("RL"),this.$={stmt:"dir",value:"RL"};break;case 41:m.setDirection("LR"),this.$={stmt:"dir",value:"LR"};break;case 44:case 45:this.$={stmt:"state",id:r[u].trim(),type:"default",description:""};break;case 46:this.$={stmt:"state",id:r[u-2].trim(),classes:[r[u].trim()],type:"default",description:""};break;case 47:this.$={stmt:"state",id:r[u-2].trim(),classes:[r[u].trim()],type:"default",description:""};break}},"anonymous"),table:[{3:1,4:e,5:s,6:a},{1:[3]},{3:5,4:e,5:s,6:a},{3:6,4:e,5:s,6:a},t([1,4,5,16,17,19,22,24,25,26,27,28,29,33,35,37,38,41,45,48,51,52,53,54,57],i,{7:7}),{1:[2,1]},{1:[2,2]},{1:[2,3],4:n,5:l,8:8,9:10,10:12,11:13,12:14,13:15,16:d,17:h,19:y,22:T,24:_,25:I,26:x,27:L,28:N,29:$,32:25,33:D,35:g,37:v,38:F,41:Y,45:C,48:G,51:P,52:ot,53:lt,54:ct,57:J},t(S,[2,5]),{9:39,10:12,11:13,12:14,13:15,16:d,17:h,19:y,22:T,24:_,25:I,26:x,27:L,28:N,29:$,32:25,33:D,35:g,37:v,38:F,41:Y,45:C,48:G,51:P,52:ot,53:lt,54:ct,57:J},t(S,[2,7]),t(S,[2,8]),t(S,[2,9]),t(S,[2,10]),t(S,[2,11]),t(S,[2,12],{14:[1,40],15:[1,41]}),t(S,[2,16]),{18:[1,42]},t(S,[2,18],{20:[1,43]}),{23:[1,44]},t(S,[2,22]),t(S,[2,23]),t(S,[2,24]),t(S,[2,25]),{30:45,31:[1,46],59:[1,47],60:[1,48]},t(S,[2,28]),{34:[1,49]},{36:[1,50]},t(S,[2,31]),{13:51,24:_,57:J},{42:[1,52],44:[1,53]},{46:[1,54]},{49:[1,55]},t(ht,[2,44],{58:[1,56]}),t(ht,[2,45],{58:[1,57]}),t(S,[2,38]),t(S,[2,39]),t(S,[2,40]),t(S,[2,41]),t(S,[2,6]),t(S,[2,13]),{13:58,24:_,57:J},t(S,[2,17]),t(It,i,{7:59}),{24:[1,60]},{24:[1,61]},{23:[1,62]},{24:[2,48]},{24:[2,49]},t(S,[2,29]),t(S,[2,30]),{39:[1,63],40:[1,64]},{43:[1,65]},{43:[1,66]},{47:[1,67]},{50:[1,68]},{24:[1,69]},{24:[1,70]},t(S,[2,14],{14:[1,71]}),{4:n,5:l,8:8,9:10,10:12,11:13,12:14,13:15,16:d,17:h,19:y,21:[1,72],22:T,24:_,25:I,26:x,27:L,28:N,29:$,32:25,33:D,35:g,37:v,38:F,41:Y,45:C,48:G,51:P,52:ot,53:lt,54:ct,57:J},t(S,[2,20],{20:[1,73]}),{31:[1,74]},{24:[1,75]},{39:[1,76]},{39:[1,77]},t(S,[2,34]),t(S,[2,35]),t(S,[2,36]),t(S,[2,37]),t(ht,[2,46]),t(ht,[2,47]),t(S,[2,15]),t(S,[2,19]),t(It,i,{7:78}),t(S,[2,26]),t(S,[2,27]),{5:[1,79]},{5:[1,80]},{4:n,5:l,8:8,9:10,10:12,11:13,12:14,13:15,16:d,17:h,19:y,21:[1,81],22:T,24:_,25:I,26:x,27:L,28:N,29:$,32:25,33:D,35:g,37:v,38:F,41:Y,45:C,48:G,51:P,52:ot,53:lt,54:ct,57:J},t(S,[2,32]),t(S,[2,33]),t(S,[2,21])],defaultActions:{5:[2,1],6:[2,2],47:[2,48],48:[2,49]},parseError:p(function(c,f){if(f.recoverable)this.trace(c);else{var o=new Error(c);throw o.hash=f,o}},"parseError"),parse:p(function(c){var f=this,o=[0],m=[],E=[null],r=[],V=this.table,u="",M=0,q=0,ut=2,et=1,mt=r.slice.call(arguments,1),k=Object.create(this.lexer),H={yy:{}};for(var Et in this.yy)Object.prototype.hasOwnProperty.call(this.yy,Et)&&(H.yy[Et]=this.yy[Et]);k.setInput(c,H.yy),H.yy.lexer=k,H.yy.parser=this,typeof k.yylloc>"u"&&(k.yylloc={});var _t=k.yylloc;r.push(_t);var ee=k.options&&k.options.ranges;typeof H.yy.parseError=="function"?this.parseError=H.yy.parseError:this.parseError=Object.getPrototypeOf(this).parseError;function se(R){o.length=o.length-2*R,E.length=E.length-R,r.length=r.length-R}p(se,"popStack");function Rt(){var R;return R=m.pop()||k.lex()||et,typeof R!="number"&&(R instanceof Array&&(m=R,R=m.pop()),R=f.symbols_[R]||R),R}p(Rt,"lex");for(var w,z,O,bt,Q={},ft,W,Ot,pt;;){if(z=o[o.length-1],this.defaultActions[z]?O=this.defaultActions[z]:((w===null||typeof w>"u")&&(w=Rt()),O=V[z]&&V[z][w]),typeof O>"u"||!O.length||!O[0]){var kt="";pt=[];for(ft in V[z])this.terminals_[ft]&&ft>ut&&pt.push("'"+this.terminals_[ft]+"'");k.showPosition?kt="Parse error on line "+(M+1)+`:
`+k.showPosition()+`
Expecting `+pt.join(", ")+", got '"+(this.terminals_[w]||w)+"'":kt="Parse error on line "+(M+1)+": Unexpected "+(w==et?"end of input":"'"+(this.terminals_[w]||w)+"'"),this.parseError(kt,{text:k.match,token:this.terminals_[w]||w,line:k.yylineno,loc:_t,expected:pt})}if(O[0]instanceof Array&&O.length>1)throw new Error("Parse Error: multiple actions possible at state: "+z+", token: "+w);switch(O[0]){case 1:o.push(w),E.push(k.yytext),r.push(k.yylloc),o.push(O[1]),w=null,q=k.yyleng,u=k.yytext,M=k.yylineno,_t=k.yylloc;break;case 2:if(W=this.productions_[O[1]][1],Q.$=E[E.length-W],Q._$={first_line:r[r.length-(W||1)].first_line,last_line:r[r.length-1].last_line,first_column:r[r.length-(W||1)].first_column,last_column:r[r.length-1].last_column},ee&&(Q._$.range=[r[r.length-(W||1)].range[0],r[r.length-1].range[1]]),bt=this.performAction.apply(Q,[u,q,M,H.yy,O[1],E,r].concat(mt)),typeof bt<"u")return bt;W&&(o=o.slice(0,-1*W*2),E=E.slice(0,-1*W),r=r.slice(0,-1*W)),o.push(this.productions_[O[1]][0]),E.push(Q.$),r.push(Q._$),Ot=V[o[o.length-2]][o[o.length-1]],o.push(Ot);break;case 3:return!0}}return!0},"parse")},te=(function(){var j={EOF:1,parseError:p(function(f,o){if(this.yy.parser)this.yy.parser.parseError(f,o);else throw new Error(f)},"parseError"),setInput:p(function(c,f){return this.yy=f||this.yy||{},this._input=c,this._more=this._backtrack=this.done=!1,this.yylineno=this.yyleng=0,this.yytext=this.matched=this.match="",this.conditionStack=["INITIAL"],this.yylloc={first_line:1,first_column:0,last_line:1,last_column:0},this.options.ranges&&(this.yylloc.range=[0,0]),this.offset=0,this},"setInput"),input:p(function(){var c=this._input[0];this.yytext+=c,this.yyleng++,this.offset++,this.match+=c,this.matched+=c;var f=c.match(/(?:\r\n?|\n).*/g);return f?(this.yylineno++,this.yylloc.last_line++):this.yylloc.last_column++,this.options.ranges&&this.yylloc.range[1]++,this._input=this._input.slice(1),c},"input"),unput:p(function(c){var f=c.length,o=c.split(/(?:\r\n?|\n)/g);this._input=c+this._input,this.yytext=this.yytext.substr(0,this.yytext.length-f),this.offset-=f;var m=this.match.split(/(?:\r\n?|\n)/g);this.match=this.match.substr(0,this.match.length-1),this.matched=this.matched.substr(0,this.matched.length-1),o.length-1&&(this.yylineno-=o.length-1);var E=this.yylloc.range;return this.yylloc={first_line:this.yylloc.first_line,last_line:this.yylineno+1,first_column:this.yylloc.first_column,last_column:o?(o.length===m.length?this.yylloc.first_column:0)+m[m.length-o.length].length-o[0].length:this.yylloc.first_column-f},this.options.ranges&&(this.yylloc.range=[E[0],E[0]+this.yyleng-f]),this.yyleng=this.yytext.length,this},"unput"),more:p(function(){return this._more=!0,this},"more"),reject:p(function(){if(this.options.backtrack_lexer)this._backtrack=!0;else return this.parseError("Lexical error on line "+(this.yylineno+1)+`. You can only invoke reject() in the lexer when the lexer is of the backtracking persuasion (options.backtrack_lexer = true).
`+this.showPosition(),{text:"",token:null,line:this.yylineno});return this},"reject"),less:p(function(c){this.unput(this.match.slice(c))},"less"),pastInput:p(function(){var c=this.matched.substr(0,this.matched.length-this.match.length);return(c.length>20?"...":"")+c.substr(-20).replace(/\n/g,"")},"pastInput"),upcomingInput:p(function(){var c=this.match;return c.length<20&&(c+=this._input.substr(0,20-c.length)),(c.substr(0,20)+(c.length>20?"...":"")).replace(/\n/g,"")},"upcomingInput"),showPosition:p(function(){var c=this.pastInput(),f=new Array(c.length+1).join("-");return c+this.upcomingInput()+`
`+f+"^"},"showPosition"),test_match:p(function(c,f){var o,m,E;if(this.options.backtrack_lexer&&(E={yylineno:this.yylineno,yylloc:{first_line:this.yylloc.first_line,last_line:this.last_line,first_column:this.yylloc.first_column,last_column:this.yylloc.last_column},yytext:this.yytext,match:this.match,matches:this.matches,matched:this.matched,yyleng:this.yyleng,offset:this.offset,_more:this._more,_input:this._input,yy:this.yy,conditionStack:this.conditionStack.slice(0),done:this.done},this.options.ranges&&(E.yylloc.range=this.yylloc.range.slice(0))),m=c[0].match(/(?:\r\n?|\n).*/g),m&&(this.yylineno+=m.length),this.yylloc={first_line:this.yylloc.last_line,last_line:this.yylineno+1,first_column:this.yylloc.last_column,last_column:m?m[m.length-1].length-m[m.length-1].match(/\r?\n?/)[0].length:this.yylloc.last_column+c[0].length},this.yytext+=c[0],this.match+=c[0],this.matches=c,this.yyleng=this.yytext.length,this.options.ranges&&(this.yylloc.range=[this.offset,this.offset+=this.yyleng]),this._more=!1,this._backtrack=!1,this._input=this._input.slice(c[0].length),this.matched+=c[0],o=this.performAction.call(this,this.yy,this,f,this.conditionStack[this.conditionStack.length-1]),this.done&&this._input&&(this.done=!1),o)return o;if(this._backtrack){for(var r in E)this[r]=E[r];return!1}return!1},"test_match"),next:p(function(){if(this.done)return this.EOF;this._input||(this.done=!0);var c,f,o,m;this._more||(this.yytext="",this.match="");for(var E=this._currentRules(),r=0;r<E.length;r++)if(o=this._input.match(this.rules[E[r]]),o&&(!f||o[0].length>f[0].length)){if(f=o,m=r,this.options.backtrack_lexer){if(c=this.test_match(o,E[r]),c!==!1)return c;if(this._backtrack){f=!1;continue}else return!1}else if(!this.options.flex)break}return f?(c=this.test_match(f,E[m]),c!==!1?c:!1):this._input===""?this.EOF:this.parseError("Lexical error on line "+(this.yylineno+1)+`. Unrecognized text.
`+this.showPosition(),{text:"",token:null,line:this.yylineno})},"next"),lex:p(function(){var f=this.next();return f||this.lex()},"lex"),begin:p(function(f){this.conditionStack.push(f)},"begin"),popState:p(function(){var f=this.conditionStack.length-1;return f>0?this.conditionStack.pop():this.conditionStack[0]},"popState"),_currentRules:p(function(){return this.conditionStack.length&&this.conditionStack[this.conditionStack.length-1]?this.conditions[this.conditionStack[this.conditionStack.length-1]].rules:this.conditions.INITIAL.rules},"_currentRules"),topState:p(function(f){return f=this.conditionStack.length-1-Math.abs(f||0),f>=0?this.conditionStack[f]:"INITIAL"},"topState"),pushState:p(function(f){this.begin(f)},"pushState"),stateStackSize:p(function(){return this.conditionStack.length},"stateStackSize"),options:{"case-insensitive":!0},performAction:p(function(f,o,m,E){function r(){const V=o.yytext.indexOf("%%");if(V===0)return!1;if(V>0){const u=o.yytext.slice(0,V),M=o.yytext.slice(V);M&&f.lexer.unput(M),o.yytext=u}return!0}switch(p(r,"processId"),m){case 0:return 38;case 1:return 40;case 2:return 39;case 3:return 44;case 4:return 51;case 5:return 52;case 6:return 53;case 7:return 54;case 8:return 5;case 9:break;case 10:break;case 11:break;case 12:break;case 13:return this.pushState("SCALE"),17;case 14:return 18;case 15:this.popState();break;case 16:return this.begin("acc_title"),33;case 17:return this.popState(),"acc_title_value";case 18:return this.begin("acc_descr"),35;case 19:return this.popState(),"acc_descr_value";case 20:this.begin("acc_descr_multiline");break;case 21:this.popState();break;case 22:return"acc_descr_multiline_value";case 23:return this.pushState("CLASSDEF"),41;case 24:return this.popState(),this.pushState("CLASSDEFID"),"DEFAULT_CLASSDEF_ID";case 25:return this.popState(),this.pushState("CLASSDEFID"),42;case 26:return this.popState(),43;case 27:return this.pushState("CLASS"),48;case 28:return this.popState(),this.pushState("CLASS_STYLE"),49;case 29:return this.popState(),50;case 30:return this.pushState("STYLE"),45;case 31:return this.popState(),this.pushState("STYLEDEF_STYLES"),46;case 32:return this.popState(),47;case 33:return this.pushState("SCALE"),17;case 34:return 18;case 35:this.popState();break;case 36:this.pushState("STATE");break;case 37:return this.popState(),o.yytext=o.yytext.slice(0,-8).trim(),25;case 38:return this.popState(),o.yytext=o.yytext.slice(0,-8).trim(),26;case 39:return this.popState(),o.yytext=o.yytext.slice(0,-10).trim(),27;case 40:return this.popState(),o.yytext=o.yytext.slice(0,-8).trim(),25;case 41:return this.popState(),o.yytext=o.yytext.slice(0,-8).trim(),26;case 42:return this.popState(),o.yytext=o.yytext.slice(0,-10).trim(),27;case 43:return 51;case 44:return 52;case 45:return 53;case 46:return 54;case 47:this.pushState("STATE_STRING");break;case 48:return this.pushState("STATE_ID"),"AS";case 49:return r()?(this.popState(),"ID"):void 0;case 50:this.popState();break;case 51:return"STATE_DESCR";case 52:throw new Error('Error: State name must be a single word. Found: "'+o.yytext.trim()+'"');case 53:return 19;case 54:this.popState();break;case 55:return this.popState(),this.pushState("struct"),20;case 56:return this.popState(),21;case 57:break;case 58:return this.begin("NOTE"),29;case 59:return this.popState(),this.pushState("NOTE_ID"),59;case 60:return this.popState(),this.pushState("NOTE_ID"),60;case 61:this.popState(),this.pushState("FLOATING_NOTE");break;case 62:return this.popState(),this.pushState("FLOATING_NOTE_ID"),"AS";case 63:break;case 64:return"NOTE_TEXT";case 65:return r()?(this.popState(),"ID"):void 0;case 66:return r()?(this.popState(),this.pushState("NOTE_TEXT"),24):void 0;case 67:return this.popState(),o.yytext=o.yytext.substr(2).trim(),31;case 68:return this.popState(),o.yytext=o.yytext.slice(0,-8).trim(),31;case 69:return 6;case 70:return 6;case 71:return 16;case 72:return 57;case 73:return r()?24:void 0;case 74:return o.yytext=o.yytext.trim(),14;case 75:return 15;case 76:return 28;case 77:return 58;case 78:return 5;case 79:return"INVALID"}},"anonymous"),rules:[/^(?:click\b)/i,/^(?:href\b)/i,/^(?:"[^"]*")/i,/^(?:default\b)/i,/^(?:.*direction\s+TB[^\n]*)/i,/^(?:.*direction\s+BT[^\n]*)/i,/^(?:.*direction\s+RL[^\n]*)/i,/^(?:.*direction\s+LR[^\n]*)/i,/^(?:[\n]+)/i,/^(?:[\s]+)/i,/^(?:((?!\n)\s)+)/i,/^(?:#[^\n]*)/i,/^(?:%%(?!\{)[^\n]*)/i,/^(?:scale\s+)/i,/^(?:\d+)/i,/^(?:\s+width\b)/i,/^(?:accTitle\s*:\s*)/i,/^(?:(?!\n||)*[^\n]*)/i,/^(?:accDescr\s*:\s*)/i,/^(?:(?!\n||)*[^\n]*)/i,/^(?:accDescr\s*\{\s*)/i,/^(?:[\}])/i,/^(?:[^\}]*)/i,/^(?:classDef\s+)/i,/^(?:DEFAULT\s+)/i,/^(?:\w+\s+)/i,/^(?:[^\n]*)/i,/^(?:class\s+)/i,/^(?:(\w+)+((,\s*\w+)*))/i,/^(?:[^\n]*)/i,/^(?:style\s+)/i,/^(?:[\w,]+\s+)/i,/^(?:[^\n]*)/i,/^(?:scale\s+)/i,/^(?:\d+)/i,/^(?:\s+width\b)/i,/^(?:state\s+)/i,/^(?:.*<<fork>>)/i,/^(?:.*<<join>>)/i,/^(?:.*<<choice>>)/i,/^(?:.*\[\[fork\]\])/i,/^(?:.*\[\[join\]\])/i,/^(?:.*\[\[choice\]\])/i,/^(?:.*direction\s+TB[^\n]*)/i,/^(?:.*direction\s+BT[^\n]*)/i,/^(?:.*direction\s+RL[^\n]*)/i,/^(?:.*direction\s+LR[^\n]*)/i,/^(?:["])/i,/^(?:\s*as\s+)/i,/^(?:[^\n\{]*)/i,/^(?:["])/i,/^(?:[^"]*)/i,/^(?:\w+\s+\w+.*?\{)/i,/^(?:[^\n\s\{]+)/i,/^(?:\n)/i,/^(?:\{)/i,/^(?:\})/i,/^(?:[\n])/i,/^(?:note\s+)/i,/^(?:left of\b)/i,/^(?:right of\b)/i,/^(?:")/i,/^(?:\s*as\s*)/i,/^(?:["])/i,/^(?:[^"]*)/i,/^(?:[^\n]*)/i,/^(?:\s*[^:\n\s\-]+)/i,/^(?:\s*:[^:\n;]+)/i,/^(?:[\s\S]*?\n\s*end note\b)/i,/^(?:stateDiagram\s+)/i,/^(?:stateDiagram-v2\s+)/i,/^(?:hide empty description\b)/i,/^(?:\[\*\])/i,/^(?:[^:\n\s\-\{]+)/i,/^(?:\s*:(?:[^:\n;]|:[^:\n;])+)/i,/^(?:-->)/i,/^(?:--)/i,/^(?::::)/i,/^(?:$)/i,/^(?:.)/i],conditions:{LINE:{rules:[10,11,12],inclusive:!1},struct:{rules:[10,11,12,23,27,30,36,43,44,45,46,56,57,58,72,73,74,75,76,77],inclusive:!1},FLOATING_NOTE_ID:{rules:[65],inclusive:!1},FLOATING_NOTE:{rules:[62,63,64],inclusive:!1},NOTE_TEXT:{rules:[67,68],inclusive:!1},NOTE_ID:{rules:[66],inclusive:!1},NOTE:{rules:[59,60,61],inclusive:!1},STYLEDEF_STYLEOPTS:{rules:[],inclusive:!1},STYLEDEF_STYLES:{rules:[32],inclusive:!1},STYLE_IDS:{rules:[],inclusive:!1},STYLE:{rules:[31],inclusive:!1},CLASS_STYLE:{rules:[29],inclusive:!1},CLASS:{rules:[28],inclusive:!1},CLASSDEFID:{rules:[26],inclusive:!1},CLASSDEF:{rules:[24,25],inclusive:!1},acc_descr_multiline:{rules:[21,22],inclusive:!1},acc_descr:{rules:[19],inclusive:!1},acc_title:{rules:[17],inclusive:!1},SCALE:{rules:[14,15,34,35],inclusive:!1},ALIAS:{rules:[],inclusive:!1},STATE_ID:{rules:[49],inclusive:!1},STATE_STRING:{rules:[50,51],inclusive:!1},FORK_STATE:{rules:[],inclusive:!1},STATE:{rules:[10,11,12,37,38,39,40,41,42,47,48,52,53,54,55],inclusive:!1},ID:{rules:[10,11,12],inclusive:!1},INITIAL:{rules:[0,1,2,3,4,5,6,7,8,9,11,12,13,16,18,20,23,27,30,33,36,55,58,69,70,71,72,73,74,75,77,78,79],inclusive:!0}}};return j})();Tt.lexer=te;function dt(){this.yy={}}return p(dt,"Parser"),dt.prototype=Tt,Tt.Parser=dt,new dt})();At.parser=At;var be=At,ke="TB",Gt="TB",Nt="dir",tt="state",Z="root",xt="relation",ve="classDef",De="style",Ce="applyClass",at="default",Vt="divider",Mt="fill:none",Wt="fill: #333",jt="c",Ut="markdown",Ht="normal",Dt="rect",Ct="rectWithTitle",Ae="stateStart",xe="stateEnd",Lt="divider",$t="roundedWithTitle",Le="note",we="noteGroup",nt="statediagram",Ie="state",Re=`${nt}-${Ie}`,zt="transition",Oe="note",Ne="note-edge",$e=`${zt} ${Ne}`,Fe=`${nt}-${Oe}`,Pe="cluster",Be=`${nt}-${Pe}`,Ye="cluster-alt",Ge=`${nt}-${Ye}`,Kt="parent",Xt="note",Ve="state",wt="----",Me=`${wt}${Xt}`,Ft=`${wt}${Kt}`,St=new Map,U=0,Jt=0,st=new Map,We=p((t,e,s,a)=>{if(t===Lt&&(s==null?void 0:s.id)!==void 0&&st.has(s.id)){const l=st.get(s.id);return st.set(e,l),l}const i=Jt++,n=a?void 0:i;return st.set(e,n),n},"colorSlotFor");function yt(t="",e=0,s="",a=wt){const i=s!==null&&s.length>0?`${a}${s}`:"";return`${Ve}-${t}${i}-${e}`}p(yt,"stateDomId");var je=p((t,e,s,a,i,n,l,d)=>{b.trace("items",e),e.forEach(h=>{switch(h.stmt){case tt:rt(t,h,s,a,i,n,l,d);break;case at:rt(t,h,s,a,i,n,l,d);break;case xt:{rt(t,h.state1,s,a,i,n,l,d),rt(t,h.state2,s,a,i,n,l,d);const y=l==="neo",T={id:"edge"+U,start:h.state1.id,end:h.state2.id,arrowhead:"normal",arrowTypeEnd:y?"arrow_barb_neo":"arrow_barb",style:Mt,labelStyle:"",label:K.sanitizeText(h.description??"",B()),arrowheadStyle:Wt,labelpos:jt,labelType:Ut,thickness:Ht,classes:zt,look:l};i.push(T),U++}break}})},"setupDoc"),Pt=p((t,e=Gt)=>{let s=e;if(t.doc)for(const a of t.doc)a.stmt==="dir"&&(s=a.value);return s},"getDir");function it(t,e,s){if(!e.id||e.id==="</join></fork>"||e.id==="</choice>")return;e.cssClasses&&(Array.isArray(e.cssCompiledStyles)||(e.cssCompiledStyles=[]),e.cssClasses.split(" ").forEach(i=>{const n=s.get(i);n&&(e.cssCompiledStyles=[...e.cssCompiledStyles??[],...n.styles])}));const a=t.find(i=>i.id===e.id);a?Object.assign(a,e):t.push(e)}p(it,"insertOrUpdateNode");function qt(t){var e;return((e=t==null?void 0:t.classes)==null?void 0:e.join(" "))??""}p(qt,"getClassesFromDbInfo");function Qt(t){return(t==null?void 0:t.styles)??[]}p(Qt,"getStylesFromDbInfo");var rt=p((t,e,s,a,i,n,l,d)=>{var L,N,$;const h=e.id,y=s.get(h),T=qt(y),_=Qt(y),I=B(),x=T.trim()!==""||_.length>0;if(b.info("dataFetcher parsedItem",e,y,_),h!=="root"){let D=Dt;e.start===!0?D=Ae:e.start===!1&&(D=xe),e.type!==at&&(D=e.type),St.get(h)||St.set(h,{id:h,shape:D,description:K.sanitizeText(h,I),cssClasses:`${T} ${Re}`,cssStyles:_});const g=St.get(h);e.description&&(Array.isArray(g.description)?(g.shape=Ct,g.description.push(e.description)):(L=g.description)!=null&&L.length&&g.description.length>0?(g.shape=Ct,g.description===h?g.description=[e.description]:g.description=[g.description,e.description]):(g.shape=Dt,g.description=e.description),g.description=K.sanitizeTextOrArray(g.description,I)),((N=g.description)==null?void 0:N.length)===1&&g.shape===Ct&&(g.type==="group"?g.shape=$t:g.shape=Dt),!g.type&&e.doc&&(b.info("Setting cluster for XCX",h,Pt(e)),g.type="group",g.isGroup=!0,g.dir=Pt(e),g.shape=e.type===Vt?Lt:$t,g.colorIndex=We(g.shape,h,t,x),g.cssClasses=`${g.cssClasses} ${Be} ${n?Ge:""}`);const v={labelStyle:"",shape:g.shape,label:g.description,cssClasses:g.cssClasses,cssCompiledStyles:[],cssStyles:g.cssStyles,id:h,dir:g.dir,domId:yt(h,U),type:g.type,isGroup:g.type==="group",colorIndex:g.colorIndex,padding:8,rx:10,ry:10,look:l,labelType:"markdown"};if(v.shape===Lt&&(v.label=""),t&&t.id!=="root"&&(b.trace("Setting node ",h," to be child of its parent ",t.id),v.parentId=t.id),v.centerLabel=!0,e.note){const F={labelStyle:"",shape:Le,label:e.note.text,labelType:"markdown",cssClasses:Fe,cssStyles:[],cssCompiledStyles:[],id:h+Me+"-"+U,domId:yt(h,U,Xt),type:"node",isGroup:!1,padding:($=I.flowchart)==null?void 0:$.padding,look:l,position:e.note.position},Y=h+Ft,C={labelStyle:"",shape:we,label:e.note.text,cssClasses:g.cssClasses,cssStyles:[],id:h+Ft,domId:yt(h,U,Kt),type:"group",isGroup:!0,padding:16,look:l,position:e.note.position};U++,C.id=Y,F.parentId=Y,it(a,C,d),it(a,F,d),it(a,v,d);let G=h,P=F.id;e.note.position==="left of"&&(G=F.id,P=h),i.push({id:G+"-"+P,start:G,end:P,arrowhead:"none",arrowTypeEnd:"",style:Mt,labelStyle:"",classes:$e,pattern:"dashed",arrowheadStyle:Wt,labelpos:jt,labelType:Ut,thickness:Ht,look:l})}else it(a,v,d)}e.doc&&(b.trace("Adding nodes children "),je(e,e.doc,s,a,i,!n,l,d))},"dataFetcher"),Ue=p(()=>{St.clear(),U=0,Jt=0,st.clear()},"reset"),Zt=p((t,e=Gt)=>{if(!t.doc)return e;let s=e;for(const a of t.doc)a.stmt==="dir"&&(s=a.value);return s},"getDir"),He=p(function(t,e){return e.db.getClasses()},"getClasses"),ze=p(async function(t,e,s,a){b.info("REF0:"),b.info("Drawing state diagram (v2)",e);const{securityLevel:i,state:n,layout:l}=B();a.db.extract(a.db.getRootDocV2());const d=a.db.getData(),h=ie(e,i);d.type=a.type,d.layoutAlgorithm=ae(l),d.nodeSpacing=(n==null?void 0:n.nodeSpacing)||50,d.rankSpacing=(n==null?void 0:n.rankSpacing)||50,B().look==="neo"?d.markers=["barbNeo"]:d.markers=["barb"],d.diagramId=e,await ne(d,h);const T=8;try{(typeof a.db.getLinks=="function"?a.db.getLinks():new Map).forEach((I,x)=>{var Y;const L=typeof x=="string"?x:typeof(x==null?void 0:x.id)=="string"?x.id:"",N=d.nodes.find(C=>C.id===L);if(!L){b.warn("⚠️ Invalid or missing stateId from key:",JSON.stringify(x));return}const $=(Y=h.node())==null?void 0:Y.querySelectorAll("g.node, g.rough-node");let D;if($==null||$.forEach(C=>{var P;const G=(P=C.textContent)==null?void 0:P.trim();(C.id===(N==null?void 0:N.domId)||G===L)&&(D=C)}),!D){b.warn("⚠️ Could not find node matching text:",L);return}const g=D.parentNode;if(!g){b.warn("⚠️ Node has no parent, cannot wrap:",L);return}const v=document.createElementNS("http://www.w3.org/2000/svg","a"),F=I.url.replace(/^"+|"+$/g,"");if(v.setAttributeNS("http://www.w3.org/1999/xlink","xlink:href",F),v.setAttribute("target","_blank"),I.tooltip){const C=I.tooltip.replace(/^"+|"+$/g,"");v.setAttribute("title",C),D.setAttribute("title",C)}g.replaceChild(v,D),v.appendChild(D),b.info("🔗 Wrapped node in <a> tag for:",L,I.url)})}catch(_){b.error("❌ Error injecting clickable links:",_)}oe.insertTitle(h,"statediagramTitleText",(n==null?void 0:n.titleTopMargin)??25,a.db.getDiagramTitle()),re(h,T,nt,(n==null?void 0:n.useMaxWidth)??!0)},"draw"),Ke={getClasses:He,draw:ze,getDir:Zt},A={START_NODE:"[*]",START_TYPE:"start",END_NODE:"[*]",END_TYPE:"end",COLOR_KEYWORD:"color",FILL_KEYWORD:"fill",BG_FILL:"bgFill",STYLECLASS_SEP:","},Bt=p(()=>new Map,"newClassesList"),Yt=p(()=>({relations:[],states:new Map,documents:{}}),"newDoc"),gt=p(t=>JSON.parse(JSON.stringify(t)),"clone"),X,Xe=(X=class{constructor(e){this.version=e,this.nodes=[],this.edges=[],this.rootDoc=[],this.classes=Bt(),this.documents={root:Yt()},this.currentDocument=this.documents.root,this.startEndCount=0,this.dividerCnt=0,this.links=new Map,this.funs=[],this.getAccTitle=le,this.setAccTitle=ce,this.getAccDescription=he,this.setAccDescription=de,this.setDiagramTitle=ue,this.getDiagramTitle=fe,this.clear(),this.setRootDoc=this.setRootDoc.bind(this),this.getDividerId=this.getDividerId.bind(this),this.setDirection=this.setDirection.bind(this),this.trimColon=this.trimColon.bind(this),this.bindFunctions=this.bindFunctions.bind(this)}extract(e){this.clear(!0);for(const i of Array.isArray(e)?e:e.doc)switch(i.stmt){case tt:this.addState(i.id.trim(),i.type,i.doc,i.description,i.note);break;case xt:this.addRelation(i.state1,i.state2,i.description);break;case ve:this.addStyleClass(i.id.trim(),i.classes);break;case De:this.handleStyleDef(i);break;case Ce:this.setCssClass(i.id.trim(),i.styleClass);break;case"click":this.addLink(i.id,i.url,i.tooltip);break}const s=this.getStates(),a=B();Ue(),rt(void 0,this.getRootDocV2(),s,this.nodes,this.edges,!0,a.look,this.classes);for(const i of this.nodes)if(Array.isArray(i.label)){if(i.description=i.label.slice(1),i.isGroup&&i.description.length>0)throw new Error(`Group nodes can only have label. Remove the additional description for node [${i.id}]`);i.label=i.label[0]}}handleStyleDef(e){const s=e.id.trim().split(","),a=e.styleClass.split(",");for(const i of s){let n=this.getState(i);if(!n){const l=i.trim();this.addState(l),n=this.getState(l)}n&&(n.styles=a.map(l=>{var d;return(d=l.replace(/;/g,""))==null?void 0:d.trim()}))}}setRootDoc(e){b.info("Setting root doc",e),this.rootDoc=e,this.version===1?this.extract(e):this.extract(this.getRootDocV2())}docTranslator(e,s,a){if(s.stmt===xt){this.docTranslator(e,s.state1,!0),this.docTranslator(e,s.state2,!1);return}if(s.stmt===tt&&(s.id===A.START_NODE?(s.id=e.id+(a?"_start":"_end"),s.start=a):s.id=s.id.trim()),s.stmt!==Z&&s.stmt!==tt||!s.doc)return;const i=[];let n=[];for(const l of s.doc)if(l.type===Vt){const d=gt(l);d.doc=gt(n),i.push(d),n=[]}else n.push(l);if(i.length>0&&n.length>0){const l={stmt:tt,id:pe(),type:"divider",doc:gt(n)};i.push(gt(l)),s.doc=i}s.doc.forEach(l=>this.docTranslator(s,l,!0))}getRootDocV2(){return this.docTranslator({id:Z,stmt:Z},{id:Z,stmt:Z,doc:this.rootDoc},!0),{id:Z,doc:this.rootDoc}}addState(e,s=at,a=void 0,i=void 0,n=void 0,l=void 0,d=void 0,h=void 0){const y=e==null?void 0:e.trim();if(!this.currentDocument.states.has(y))b.info("Adding state ",y,i),this.currentDocument.states.set(y,{stmt:tt,id:y,descriptions:[],type:s,doc:a,note:n,classes:[],styles:[],textStyles:[]});else{const T=this.currentDocument.states.get(y);if(!T)throw new Error(`State not found: ${y}`);T.doc||(T.doc=a),T.type||(T.type=s)}if(i&&(b.info("Setting state description",y,i),(Array.isArray(i)?i:[i]).forEach(_=>this.addDescription(y,_.trim()))),n){const T=this.currentDocument.states.get(y);if(!T)throw new Error(`State not found: ${y}`);T.note=n,T.note.text=K.sanitizeText(T.note.text,B())}l&&(b.info("Setting state classes",y,l),(Array.isArray(l)?l:[l]).forEach(_=>this.setCssClass(y,_.trim()))),d&&(b.info("Setting state styles",y,d),(Array.isArray(d)?d:[d]).forEach(_=>this.setStyle(y,_.trim()))),h&&(b.info("Setting state styles",y,d),(Array.isArray(h)?h:[h]).forEach(_=>this.setTextStyle(y,_.trim())))}clear(e){this.nodes=[],this.edges=[],this.funs=[this.setupToolTips.bind(this)],this.documents={root:Yt()},this.currentDocument=this.documents.root,this.startEndCount=0,this.classes=Bt(),e||(this.links=new Map,ge())}getState(e){return this.currentDocument.states.get(e)}getStates(){return this.currentDocument.states}logDocuments(){b.info("Documents = ",this.documents)}getRelations(){return this.currentDocument.relations}addLink(e,s,a){this.links.set(e,{url:s,tooltip:a}),b.warn("Adding link",e,s,a)}getLinks(){return this.links}startIdIfNeeded(e=""){return e===A.START_NODE?(this.startEndCount++,`${A.START_TYPE}${this.startEndCount}`):e}startTypeIfNeeded(e="",s=at){return e===A.START_NODE?A.START_TYPE:s}endIdIfNeeded(e=""){return e===A.END_NODE?(this.startEndCount++,`${A.END_TYPE}${this.startEndCount}`):e}endTypeIfNeeded(e="",s=at){return e===A.END_NODE?A.END_TYPE:s}addRelationObjs(e,s,a=""){const i=this.startIdIfNeeded(e.id.trim()),n=this.startTypeIfNeeded(e.id.trim(),e.type),l=this.startIdIfNeeded(s.id.trim()),d=this.startTypeIfNeeded(s.id.trim(),s.type);this.addState(i,n,e.doc,e.description,e.note,e.classes,e.styles,e.textStyles),this.addState(l,d,s.doc,s.description,s.note,s.classes,s.styles,s.textStyles),this.currentDocument.relations.push({id1:i,id2:l,relationTitle:K.sanitizeText(a,B())})}addRelation(e,s,a){if(typeof e=="object"&&typeof s=="object")this.addRelationObjs(e,s,a);else if(typeof e=="string"&&typeof s=="string"){const i=this.startIdIfNeeded(e.trim()),n=this.startTypeIfNeeded(e),l=this.endIdIfNeeded(s.trim()),d=this.endTypeIfNeeded(s);this.addState(i,n),this.addState(l,d),this.currentDocument.relations.push({id1:i,id2:l,relationTitle:a?K.sanitizeText(a,B()):void 0})}}addDescription(e,s){var n;const a=this.currentDocument.states.get(e),i=s.startsWith(":")?s.replace(":","").trim():s;(n=a==null?void 0:a.descriptions)==null||n.push(K.sanitizeText(i,B()))}cleanupLabel(e){return e.startsWith(":")?e.slice(2).trim():e.trim()}getDividerId(){return this.dividerCnt++,`divider-id-${this.dividerCnt}`}addStyleClass(e,s=""){this.classes.has(e)||this.classes.set(e,{id:e,styles:[],textStyles:[]});const a=this.classes.get(e);s&&a&&s.split(A.STYLECLASS_SEP).forEach(i=>{const n=i.replace(/([^;]*);/,"$1").trim();if(RegExp(A.COLOR_KEYWORD).exec(i)){const d=n.replace(A.FILL_KEYWORD,A.BG_FILL).replace(A.COLOR_KEYWORD,A.FILL_KEYWORD);a.textStyles.push(d)}a.styles.push(n)})}getClasses(){return this.classes}setupToolTips(e){const s=_e();vt(e).select("svg").selectAll("g.node, g.rough-node").on("mouseover",n=>{var y;const l=vt(n.currentTarget),d=l.attr("title");if(d===null)return;const h=(y=n.currentTarget)==null?void 0:y.getBoundingClientRect();s.transition().duration(200).style("opacity",".9"),s.style("left",window.scrollX+h.left+(h.right-h.left)/2+"px").style("top",window.scrollY+h.bottom+"px"),s.html(Se.sanitize(d)),l.classed("hover",!0)}).on("mouseout",n=>{s.transition().duration(500).style("opacity",0),vt(n.currentTarget).classed("hover",!1)})}setCssClass(e,s){e.split(",").forEach(a=>{var n;let i=this.getState(a);if(!i){const l=a.trim();this.addState(l),i=this.getState(l)}(n=i==null?void 0:i.classes)==null||n.push(s)})}setStyle(e,s){var a,i;(i=(a=this.getState(e))==null?void 0:a.styles)==null||i.push(s)}setTextStyle(e,s){var a,i;(i=(a=this.getState(e))==null?void 0:a.textStyles)==null||i.push(s)}bindFunctions(e){this.funs.forEach(s=>{s(e)})}getDirectionStatement(){return this.rootDoc.find(e=>e.stmt===Nt)}getDirection(){var e;return((e=this.getDirectionStatement())==null?void 0:e.value)??ke}setDirection(e){const s=this.getDirectionStatement();s?s.value=e:this.rootDoc.unshift({stmt:Nt,value:e})}trimColon(e){return e.startsWith(":")?e.slice(1).trim():e.trim()}getData(){var s,a;const e=B();for(const i of this.nodes)i.wrappingWidth??(i.wrappingWidth=(s=e.state)==null?void 0:s.wrappingWidth),i.isGroup||(i.minWidth??(i.minWidth=(a=e.state)==null?void 0:a.minNodeWidth));return{nodes:this.nodes,edges:this.edges,other:{},config:e,direction:Zt(this.getRootDocV2())}}getConfig(){return B().state}},p(X,"StateDB"),X.relationType={AGGREGATION:0,EXTENSION:1,COMPOSITION:2,DEPENDENCY:3},X),Je=p(t=>{const{theme:e,bkgColorArray:s,borderColorArray:a}=t;if(!ye(e,a))return"";const i=Te(t.look),n=me(s);let l="";for(let d=0;d<Ee(a);d++){const h=a[d],y=n?`fill: ${s[d%s.length]};`:"",T=`[data-look="${i}"][data-color-id="color-${d}"]`;l+=`

    /* The title strip: \`rect.outer\` spans the whole composite and \`rect.inner\` covers
       the body, so what stays visible of \`outer\` is the band behind the label. */
    ${T}.statediagram-cluster rect.outer {
      stroke: ${h};
      ${y}
    }

    ${T}.statediagram-cluster rect.inner {
      stroke: ${h};
    }

    /* Concurrency regions. Siblings of one composite share a slot, so a divided composite
       reads as one thing split into parts rather than as several composites. */
    ${T}.statediagram-cluster rect.divider {
      stroke: ${h};
      ${y}
    }

    /* handDrawn draws the same container as roughjs shapes rather than plain rects, so it
       needs its own rules. \`roundedWithTitle\` and \`divider\` name those groups \`outer\`,
       \`inner\` and \`divider\` to match the classic branch, which is what lets these
       discriminate -- a bare \`.statediagram-cluster path\` rule reached the body as well and
       tinted the whole composite, losing \`compositeBackground\` and diverging from what
       classic and neo do.

       roughjs emits two paths per shape and marks them: the filled shape carries
       \`stroke="none"\` and the sketched outline carries \`fill="none"\`. Splitting on that is
       what keeps \`fill\` off the outline -- a rough outline is open squiggles, not a closed
       region, so filling it produces smears -- and keeps \`stroke\` off the fill shape, which
       would otherwise gain an edge it was drawn without. */
    ${T}.statediagram-cluster .outer path[stroke='none'] {
      ${y}
    }

    ${T}.statediagram-cluster .outer path[fill='none'] {
      stroke: ${h};
    }

    /* No \`.inner\` rule on purpose. The body shape is left entirely alone under handDrawn,
       where a rect's \`inner\` counterpart cannot be recoloured safely: roughjs draws a
       hachure fill as *stroked* lines, so its fill paths carry \`fill="none"\` exactly like
       the outline and no selector separates them. An \`.inner\` stroke rule therefore
       repainted the hatching of every alt composite in the palette colour instead of
       leaving it on \`altBackground\`. The container still reads as palette-coloured: the
       \`outer\` shape spans the whole composite, so its outline already frames the body. */

    /* Regions split the same way, which is why \`divider\` fills solid rather than taking
       roughjs's default hachure -- see the note on that call. Hatched, both of its paths
       carried \`fill="none"\` and these two rules degenerated: the tint matched nothing and
       the border rule repainted the hatching. */
    ${T}.statediagram-cluster .divider path[stroke='none'] {
      ${y}
    }

    ${T}.statediagram-cluster .divider path[fill='none'] {
      stroke: ${h};
    }
    `}return l},"genColor"),qe=p(t=>`
${Je(t)}
defs [id$="-barbEnd"] {
    fill: ${t.transitionColor};
    stroke: ${t.transitionColor};
  }
g.stateGroup text {
  fill: ${t.nodeBorder};
  stroke: none;
  font-size: 10px;
}
g.stateGroup text {
  fill: ${t.textColor};
  stroke: none;
  font-size: 10px;

}
g.stateGroup .state-title {
  font-weight: bolder;
  fill: ${t.stateLabelColor};
}

g.stateGroup rect {
  fill: ${t.mainBkg};
  stroke: ${t.nodeBorder};
}

g.stateGroup line {
  stroke: ${t.lineColor};
  stroke-width: ${t.strokeWidth||1};
}

.transition {
  stroke: ${t.transitionColor};
  stroke-width: ${t.strokeWidth||1};
  fill: none;
}

.stateGroup .composit {
  fill: ${t.background};
  border-bottom: 1px
}

.stateGroup .alt-composit {
  fill: #e0e0e0;
  border-bottom: 1px
}

.state-note {
  stroke: ${t.noteBorderColor};
  fill: ${t.noteBkgColor};

  text {
    fill: ${t.noteTextColor};
    stroke: none;
    font-size: 10px;
  }
}

.stateLabel .box {
  stroke: none;
  stroke-width: 0;
  fill: ${t.mainBkg};
  opacity: 0.5;
}

.edgeLabel .label rect {
  fill: ${t.labelBackgroundColor};
  opacity: 0.5;
}
.edgeLabel {
  background-color: ${t.edgeLabelBackground};
  p {
    background-color: ${t.edgeLabelBackground};
  }
  rect {
    opacity: 0.5;
    background-color: ${t.edgeLabelBackground};
    fill: ${t.edgeLabelBackground};
  }
  text-align: center;
}
.edgeLabel .label text {
  fill: ${t.transitionLabelColor||t.tertiaryTextColor};
}
.label div .edgeLabel {
  color: ${t.transitionLabelColor||t.tertiaryTextColor};
}

.stateLabel text {
  fill: ${t.stateLabelColor};
  font-size: 10px;
  font-weight: bold;
}

.node circle.state-start {
  fill: ${t.specialStateColor};
  stroke: ${t.specialStateColor};
}

.node .fork-join {
  fill: ${t.specialStateColor};
  stroke: ${t.specialStateColor};
}

.node circle.state-end {
  fill: ${t.innerEndBackground};
  stroke: ${t.background};
  stroke-width: 1.5
}
.end-state-inner {
  fill: ${t.compositeBackground||t.background};
  // stroke: ${t.background};
  stroke-width: 1.5
}

.node rect {
  fill: ${t.stateBkg||t.mainBkg};
  stroke: ${t.stateBorder||t.nodeBorder};
  stroke-width: ${t.strokeWidth||1}px;
}
.node polygon {
  fill: ${t.mainBkg};
  stroke: ${t.stateBorder||t.nodeBorder};;
  stroke-width: ${t.strokeWidth||1}px;
}
[id$="-barbEnd"] {
  fill: ${t.lineColor};
}

.statediagram-cluster rect {
  fill: ${t.compositeTitleBackground};
  stroke: ${t.stateBorder||t.nodeBorder};
  stroke-width: ${t.strokeWidth||1}px;
}

.cluster-label, .nodeLabel {
  color: ${t.stateLabelColor};
  // line-height: 1;
}

.statediagram-cluster rect.outer {
  rx: 5px;
  ry: 5px;
}
.statediagram-state .divider {
  stroke: ${t.stateBorder||t.nodeBorder};
}

.statediagram-state .title-state {
  rx: 5px;
  ry: 5px;
}
.statediagram-cluster.statediagram-cluster .inner {
  fill: ${t.compositeBackground||t.background};
}
.statediagram-cluster.statediagram-cluster-alt .inner {
  fill: ${t.altBackground?t.altBackground:"#efefef"};
}

.statediagram-cluster .inner {
  rx:0;
  ry:0;
}

.statediagram-state rect.basic {
  rx: 5px;
  ry: 5px;
}
.statediagram-state rect.divider {
  stroke-dasharray: 10,10;
  fill: ${t.altBackground?t.altBackground:"#efefef"};
}

.note-edge {
  stroke-dasharray: 5;
}

.statediagram-note rect {
  fill: ${t.noteBkgColor};
  stroke: ${t.noteBorderColor};
  stroke-width: 1px;
  rx: 0;
  ry: 0;
}
.statediagram-note rect {
  fill: ${t.noteBkgColor};
  stroke: ${t.noteBorderColor};
  stroke-width: 1px;
  rx: 0;
  ry: 0;
}

.statediagram-note text {
  fill: ${t.noteTextColor};
}

.statediagram-note .nodeLabel {
  color: ${t.noteTextColor};
}
.statediagram .edgeLabel {
  color: red; // ${t.noteTextColor};
}

[id$="-dependencyStart"], [id$="-dependencyEnd"] {
  fill: ${t.lineColor};
  stroke: ${t.lineColor};
  stroke-width: ${t.strokeWidth||1};
}

.statediagramTitleText {
  text-anchor: middle;
  font-size: 18px;
  fill: ${t.textColor};
}

[data-look="neo"].statediagram-cluster rect {
  fill: ${t.mainBkg};
  stroke: ${t.useGradient?"url("+t.svgId+"-gradient)":t.stateBorder||t.nodeBorder};
  stroke-width: ${t.strokeWidth??1};
}
[data-look="neo"].statediagram-cluster rect.outer {
  rx: ${t.radius}px;
  ry: ${t.radius}px;
  filter: ${t.dropShadow?t.dropShadow.replace("url(#drop-shadow)",`url(${t.svgId}-drop-shadow)`):"none"}
}
`,"getStyles"),Qe=qe,rs={parser:be,get db(){return new Xe(2)},renderer:Ke,styles:Qe,init:p(t=>{t.state||(t.state={}),t.state.arrowMarkerAbsolute=t.arrowMarkerAbsolute},"init")};export{rs as diagram};
