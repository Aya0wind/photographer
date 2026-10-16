/** Display-only smoothing. The source bins and tone-zone hit testing stay unchanged. */
export function histogramAreaPath(bins:readonly number[],peak:number,height=100,amplitude=height*.96):string {
  if(!bins.length)return "";
  const counts=bins.map(value=>Number.isFinite(value)?Math.max(0,value):0);
  const weights=[1,4,6,4,1];
  const scale=Number.isFinite(peak)&&peak>0?peak:1;
  const ys=counts.map((_,index)=>{
    const count=weights.reduce((sum,weight,offset)=>sum+counts[Math.max(0,Math.min(counts.length-1,index+offset-2))]*weight,0)/16;
    return height-Math.sqrt(Math.min(1,count/scale))*amplitude;
  });
  const dx=255/Math.max(1,ys.length-1);
  const slopes=ys.slice(1).map((y,index)=>(y-ys[index])/dx);
  // Harmonic tangents go flat at extrema, so interpolation cannot overshoot a bin.
  const tangent=(index:number)=>{
    if(index===0)return slopes[0]??0;
    if(index===ys.length-1)return slopes[index-1]??0;
    const before=slopes[index-1],after=slopes[index];
    return before*after<=0?0:2*before*after/(before+after);
  };
  const n=(value:number)=>Number(value.toFixed(3));
  let path=`M0 ${height} L0 ${n(ys[0])}`;
  for(let index=1;index<ys.length;index++){
    const x=index*dx;
    path+=` C${n(x-dx*2/3)} ${n(ys[index-1]+tangent(index-1)*dx/3)} ${n(x-dx/3)} ${n(ys[index]-tangent(index)*dx/3)} ${n(x)} ${n(ys[index])}`;
  }
  return `${path} L255 ${height} Z`;
}
