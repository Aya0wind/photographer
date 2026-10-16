interface Request {current:()=>boolean;run:()=>Promise<void>}
const pending:Request[]=[];
let running=false;
export function queueStylePreview(request:Request):void {
  for(let i=pending.length-1;i>=0;i--)if(!pending[i].current())pending.splice(i,1);
  pending.push(request);
  void pump();
}
async function pump():Promise<void> {
  if(running)return;
  running=true;
  try {
    while(pending.length){const request=pending.shift()!;if(request.current())await request.run();}
  }finally{running=false;}
}
