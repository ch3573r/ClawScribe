import assert from 'node:assert/strict';
import test from 'node:test';
import {readFileSync} from 'node:fs';
import {createRequire} from 'node:module';
import path from 'node:path';
import {loadTsModule} from './load-ts-module.mjs';

const require=createRequire(import.meta.url);
const postcss=require('postcss');
const tailwind=require('tailwindcss');
const config=require('tailwindcss/loadConfig')(path.resolve('tailwind.config.ts'));
const css=postcss.parse(readFileSync('src/app/globals.css','utf8'));
const {accentColors}=loadTsModule('src/lib/theme.ts');

function tokens(dark,accent,supportsColorMix=true) {
  const result={};
  css.walkRules(rule=>{
    for(let parent=rule.parent;parent;parent=parent.parent) {
      if(parent.type==='atrule'&&parent.name==='supports'&&parent.params.includes('color-mix')&&!supportsColorMix) return;
    }
    if(rule.selector===':root'||(dark&&rule.selector==='.dark')) {
      rule.nodes.filter(node=>node.type==='decl').forEach(node=>result[node.prop]=node.value);
    }
  });
  if(accent) result['--ring']=accent.primary;
  return result;
}
function rgb(value,variables) {
  value=value.replace(/\s*\/\s*<alpha-value>/g,'').replaceAll('<alpha-value>','1');
  value=value.replace(/var\((--[\w-]+)\)/g,(_,name)=>{
    assert.ok(variables[name],`Missing color token ${name}`);
    return variables[name];
  });
  if(value.includes('var(')) return rgb(value,variables);
  const opaqueMix=/^color-mix\(in srgb,\s*(.+)\s+calc\(1 \* 100%\),\s*transparent\)$/.exec(value);
  if(opaqueMix) return rgb(opaqueMix[1],variables);
  const mix=/^color-mix\(in srgb,\s*(.*),\s*(.*?)\s+([\d.]+)%\)$/.exec(value);
  if(mix) {
    const a=rgb(mix[1],variables),b=rgb(mix[2],variables),weight=Number(mix[3])/100;
    return a.map((channel,i)=>channel*(1-weight)+b[i]*weight);
  }
  const numbers=value.match(/[\d.]+/g)?.map(Number);
  assert.equal(numbers?.length,3,`Expected an opaque HSL color: ${value}`);
  const [h,s,l]=[numbers[0]/360,numbers[1]/100,numbers[2]/100];
  const chroma=(1-Math.abs(2*l-1))*s;
  const x=chroma*(1-Math.abs((h*6)%2-1));
  const groups=[[chroma,x,0],[x,chroma,0],[0,chroma,x],[0,x,chroma],[x,0,chroma],[chroma,0,x]];
  return groups[Math.floor(h*6)%6].map(channel=>channel+l-chroma/2);
}
const luminance=color=>color.map(value=>value<=0.04045?value/12.92:((value+0.055)/1.055)**2.4).reduce((sum,value,i)=>sum+value*[0.2126,0.7152,0.0722][i],0);
const contrast=(a,b)=>(Math.max(luminance(a),luminance(b))+0.05)/(Math.min(luminance(a),luminance(b))+0.05);

test('destructive text and button labels meet WCAG AA on both application surfaces',()=>{
  const textColor=config.theme.extend.textColor?.destructive?.DEFAULT??config.theme.extend.colors.destructive.DEFAULT;
  for(const dark of [false,true]) {
    const vars=tokens(dark);
    const label=rgb('var(--destructive-foreground)',vars),fill=rgb('var(--destructive)',vars);
    for(const surface of ['--background','--card']) {
      const background=rgb(`var(${surface})`,vars);
      const textRatio=contrast(rgb(textColor,vars),background);
      const buttonRatio=contrast(label,fill);
      const hoverRatio=contrast(label,fill.map((channel,i)=>channel*0.9+background[i]*0.1));
      console.log(`${dark?'dark':'light'} ${surface}: destructive text ${textRatio.toFixed(2)}:1; button ${buttonRatio.toFixed(2)}:1; hover ${hoverRatio.toFixed(2)}:1`);
      assert.ok(textRatio>=4.5,'Destructive text must be readable');
      assert.ok(buttonRatio>=4.5&&hoverRatio>=4.5,'Destructive button labels must be readable');
    }
  }
});

test('focus rings fall back to the selected accent without color-mix support',()=>{
  for(const dark of [false,true]) {
    for(const accent of [undefined,...accentColors]) {
      const vars=tokens(dark,accent,false);
      assert.deepEqual(rgb('var(--focus-ring)',vars),rgb('hsl(var(--ring))',vars));
    }
  }
});

test('focus rings meet 3:1 in both themes for defaults and every accent-picker choice',async()=>{
  const compiled=await postcss([tailwind({...config,content:[{raw:'ring-ring'}]})]).process('@tailwind utilities;',{from:undefined});
  let ringColor;
  compiled.root.walkRules(rule=>{
    if(rule.selector==='.ring-ring') rule.walkDecls('--tw-ring-color',declaration=>{ringColor=declaration.value;});
  });
  assert.ok(ringColor,'Opaque focus-ring utilities must compile');
  for(const dark of [false,true]) {
    const ratios=[undefined,...accentColors].flatMap(accent=>{
      const vars=tokens(dark,accent);
      return ['--background','--card'].map(surface=>{
        const ratio=contrast(rgb(ringColor,vars),rgb(`var(${surface})`,vars));
        assert.ok(ratio>=3,`${dark?'dark':'light'} ${accent?.id??'default'} ${surface}: focus ${ratio.toFixed(2)}:1`);
        return ratio;
      });
    });
    console.log(`${dark?'dark':'light'} focus, all accents and surfaces: ${Math.min(...ratios).toFixed(2)}–${Math.max(...ratios).toFixed(2)}:1`);
  }
});

test('Tailwind keeps destructive text separate from button fill, including opacity utilities',async()=>{
  const result=await postcss([tailwind({...config,content:[{raw:'text-destructive text-destructive/80 text-destructive-foreground bg-destructive bg-destructive/90 ring-ring ring-ring/50 focus-visible:ring-2 focus-visible:ring-ring'}]})]).process('@tailwind utilities;',{from:undefined});
  const declarations={};
  result.root.walkRules(rule=>declarations[rule.selector]=rule.toString());
  assert.match(declarations['.text-destructive'],/--destructive-text/);
  assert.match(declarations['.text-destructive\\/80'],/--destructive-text/);
  assert.match(declarations['.bg-destructive'],/var\(--destructive\)/);
  assert.match(declarations['.text-destructive-foreground'],/--destructive-foreground/);
  assert.match(declarations['.ring-ring'],/--focus-ring/);
  assert.doesNotMatch(declarations['.ring-ring'],/color-mix/,'Opaque rings must remain valid without color-mix support');
  assert.ok(declarations['.ring-ring\\/50'],'Existing focus-ring opacity utilities must still compile');
  assert.doesNotMatch(declarations['.ring-ring\\/50'],/color-mix/,'Opacity rings must remain valid without color-mix support');
  assert.match(declarations['.ring-ring\\/50'],/hsl\(var\(--ring\)\s*\/\s*0\.5\)/);
  assert.match(declarations['.focus-visible\\:ring-ring:focus-visible'],/--focus-ring/);
  assert.match(declarations['.focus-visible\\:ring-2:focus-visible'],/var\(--tw-shadow, 0 0 #0000\)/,'Focus utilities must preserve the shared shadow fallback');
});
