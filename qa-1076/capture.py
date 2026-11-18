"""Capture the terminal stream and monotonic timestamps in Asciicast v2."""
import codecs, json, os, pty, sys, time
path, args = sys.argv[1], sys.argv[2:]
with open(path, "w") as record:
    t=time.monotonic(); decoder=codecs.getincrementaldecoder("utf-8")("replace")
    record.write(json.dumps({"version":2,"width":112,"height":30,"timestamp":int(time.time()),"title":"OpenResearch #1076 | Linux end-to-end setup diagnostics"})+"\n")
    def read(fd):
        data=os.read(fd,65536)
        if data:
            record.write(json.dumps([round(time.monotonic()-t,4),"o",decoder.decode(data)])+"\n");record.flush()
        return data
    sys.exit(os.waitstatus_to_exitcode(pty.spawn(args,read)))
