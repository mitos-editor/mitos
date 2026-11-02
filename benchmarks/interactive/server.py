import sys,json,time
log=open(sys.argv[1],'w',buffering=1)
while True:
 headers={}
 while True:
  line=sys.stdin.buffer.readline()
  if not line:sys.exit(0)
  if line==b'\r\n':break
  k,v=line.decode().split(':',1);headers[k.lower()]=v.strip()
 msg=json.loads(sys.stdin.buffer.read(int(headers['content-length'])))
 method=msg.get('method','');log.write(json.dumps({'time_ns':time.monotonic_ns(),'method':method,'id':msg.get('id'),'params':msg.get('params')})+'\n')
 if 'id' not in msg:continue
 if method=='initialize':result={'capabilities':{'textDocumentSync':2,'documentHighlightProvider':True}}
 elif method=='textDocument/documentHighlight':continue # withhold responses to isolate scheduling frames
 elif method=='shutdown':result=None
 else:result=None
 body=json.dumps({'jsonrpc':'2.0','id':msg['id'],'result':result}).encode();sys.stdout.buffer.write(b'Content-Length: '+str(len(body)).encode()+b'\r\n\r\n'+body);sys.stdout.buffer.flush()
