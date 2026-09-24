# fig/bend — the command line: `bend main.bend -o fig`, then
#   fig get FILE PATH
#   fig set FILE PATH VALUE
#   fig comment FILE PATH TEXT
#   fig json5 FILE
# `set` and `comment` rewrite FILE in place; the rest print to stdout.
import Base
import ./fig.bend as Fig

def read.fin(fr: File & Result<&1, &1, U32 & String, String>) ->
  IO(Result<&1, &1, U32 & String, String>):
  (f, r) = fr
  do IO<Result<&1, &1, U32 & String, String>>:
    File.close(f)
    return r

def read(path: String) -> IO(String):
  do IO<String>:
    f : File <- IO.try(File, File.open(path, "r"))
    fr : File & Result<&1, &1, U32 & String, String> <- File.read(f, 16777216)
    text : String <- IO.try(String, read.fin(fr))
    return text

def write.fin(fr: File & Result<&1, &1, U32 & String, Unit>) ->
  IO(Result<&1, &1, U32 & String, Unit>):
  (f, r) = fr
  do IO<Result<&1, &1, U32 & String, Unit>>:
    File.close(f)
    return r

def write(path: String, text: String) -> IO(Unit):
  do IO<Unit>:
    f : File <- IO.try(File, File.open(path, "w"))
    fr : File & Result<&1, &1, U32 & String, Unit> <- File.write(f, text)
    u : Unit <- IO.try(Unit, write.fin(fr))
    return Unit{}

# Exit codes as fig documents them: 1 for the document, 2 for the command line.
def print_edit(e: Fig.Edit) -> IO(Unit):
  match e:
    case Fig.Ok{text}:
      IO.write(text)
    case Fig.Err{msg}:
      IO.die(Unit, 1, String.append("fig: ", msg))

def save_edit(path: String, e: Fig.Edit) -> IO(Unit):
  match e:
    case Fig.Ok{text}:
      write(path, text)
    case Fig.Err{msg}:
      IO.die(Unit, 1, String.append("fig: ", msg))

def usage() -> IO(Unit):
  IO.die(Unit, 2, "usage: fig get FILE PATH | set FILE PATH VALUE | comment FILE PATH TEXT | json5 FILE")

type Cmd is Data:
  Get{}
  Set{}
  Comment{}
  Json5{}
  Bad{}

def cmd.pick(c: Bool, k: Cmd, e: Cmd) -> Cmd:
  Bool.pick(Cmd, c, k, e)

def cmd(+s: String) -> Cmd:
  cmd.pick(String.eq(s, "get"), Get{},
  cmd.pick(String.eq(s, "set"), Set{},
  cmd.pick(String.eq(s, "comment"), Comment{},
  cmd.pick(String.eq(s, "json5"), Json5{}, Bad{}))))

def run2(c: Cmd, +file: String, +src: String, args: List<String>) -> IO(Unit):
  match c args:
    case Get{} Con{path, Nil{}}:
      print_edit(Fig.get(src, path))
    case Set{} Con{path, Con{value, Nil{}}}:
      save_edit(file, Fig.set(src, path, value))
    case Comment{} Con{path, Con{text, Nil{}}}:
      save_edit(file, Fig.comment(src, path, text))
    case Json5{} Nil{}:
      IO.write(Fig.json5(Fig.parse(src)))
    case _ _:
      usage()

def run.cmd(c: Cmd, +file: String, rest: List<String>) -> IO(Unit):
  match c:
    case Bad{}:
      usage()
    case _:
      do IO<Unit>:
        src : String <- read(file)
        run2(c, file, src, rest)

def run(args: List<String>) -> IO(Unit):
  match args:
    case Con{c, Con{file, rest}}:
      run.cmd(cmd(c), file, rest)
    case _:
      usage()

def main() -> IO(Unit):
  do IO<Unit>:
    args : List<String> <- IO.args()
    run(args)
