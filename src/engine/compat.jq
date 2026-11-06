# jq 1.7 builtins that jaq lacks. They only add: where jaq has a builtin,
# its behaviour is deem's.
def IN(s): any(s == .; .);
def IN(src; s): any(src == s; .);
def INDEX(stream; idx_expr): reduce stream as $row ({}; .[$row | idx_expr | tostring] |= $row);
def INDEX(idx_expr): INDEX(.[]; idx_expr);
def JOIN($idx; idx_expr): [.[] | [., $idx[idx_expr]]];
def JOIN($idx; stream; idx_expr): stream | [., $idx[idx_expr]];
def JOIN($idx; stream; idx_expr; join_expr): stream | [., $idx[idx_expr]] | join_expr;
def tostream: path(def r: (.[]? | r), .; r) as $p | getpath($p) | reduce path(.[]?) as $q ([$p, .]; [$p + $q]);
def fromstream(f): { x: null, e: false } as $init
  | foreach f as $i ($init;
      if .e then $init else . end
      | if $i | length == 2
        then setpath(["e"]; $i[0] | length == 0) | setpath(["x"] + $i[0]; $i[1])
        else setpath(["e"]; $i[0] | length == 1) end;
      if .e then .x else empty end);
def truncate_stream(stream): . as $n | null | stream | . as $input | if (.[0] | length) > $n then setpath([0]; .[0][$n:]) else empty end;
def @tsv:
  if type != "array" then error("\(type) (\(tojson)) cannot be tsv-formatted, only an array can be") end
  | map(if type == "string" then split("\\") | join("\\\\") | split("\t") | join("\\t")
          | split("\n") | join("\\n") | split("\r") | join("\\r")
        elif type == "number" or type == "boolean" then tostring
        elif . == null then ""
        else error("\(type) (\(tojson)) is not valid in a csv row") end)
  | join("\t");
def @csv:
  if type != "array" then error("\(type) (\(tojson)) cannot be csv-formatted, only an array can be") end
  | map(if type == "string" then "\"" + (split("\"") | join("\"\"")) + "\""
        elif type == "number" or type == "boolean" then tostring
        elif . == null then ""
        else error("\(type) (\(tojson)) is not valid in a csv row") end)
  | join(",");
def format($f):
  if $f == "text" then @text elif $f == "json" then @json elif $f == "csv" then @csv
  elif $f == "tsv" then @tsv elif $f == "html" then @html elif $f == "uri" then @uri
  elif $f == "sh" then @sh elif $f == "base64" then @base64 elif $f == "base64d" then @base64d
  else error("\($f) is not a valid format") end;
# jq 1.7.1's list, less what deem leaves out (input, modules), plus judge and its helpers
def builtins: ["IN/1","IN/2","INDEX/1","INDEX/2","JOIN/2","JOIN/3","JOIN/4","abs/0","acos/0","acosh/0","add/0","all/0","all/1","all/2","any/0","any/1","any/2","arrays/0","ascii_downcase/0","ascii_upcase/0","asin/0","asinh/0","ask/2","atan/0","atan2/2","atanh/0","booleans/0","box/2","bsearch/1","builtins/0","capture/1","capture/2","cbrt/0","ceil/0","choose/3","combinations/0","combinations/1","contains/1","copysign/2","cos/0","cosh/0","debug/0","debug/1","del/1","delpaths/1","drem/2","empty/0","endswith/1","env/0","erf/0","erfc/0","error/0","error/1","exp/0","exp10/0","exp2/0","explode/0","expm1/0","fabs/0","fdim/2","finites/0","first/0","first/1","flatten/0","flatten/1","floor/0","fma/3","fmax/2","fmin/2","fmod/2","format/1","frexp/0","from_entries/0","fromdate/0","fromdateiso8601/0","fromjson/0","fromstream/1","gamma/0","getpath/1","gmtime/0","group_by/1","gsub/2","gsub/3","halt/0","halt_error/0","halt_error/1","has/1","hypot/2","implode/0","in/1","index/1","indices/1","infinite/0","inside/1","isempty/1","isfinite/0","isinfinite/0","isnan/0","isnormal/0","iterables/0","j0/0","j1/0","jn/2","join/1","judge/2","keys/0","keys_unsorted/0","last/0","last/1","ldexp/2","length/0","lgamma/0","limit/2","localtime/0","log/0","log10/0","log1p/0","log2/0","logb/0","ltrimstr/1","map/1","map_values/1","match/1","match/2","max/0","max_by/1","min/0","min_by/1","mktime/0","modf/0","nan/0","nearbyint/0","nextafter/2","nexttoward/2","normals/0","not/0","now/0","nth/1","nth/2","nulls/0","numbers/0","objects/0","path/1","paths/0","paths/1","pick/1","pow/2","pow10/0","range/1","range/2","range/3","rate/3","recurse/0","recurse/1","recurse/2","remainder/2","repeat/1","reverse/0","rindex/1","rint/0","round/0","rtrimstr/1","scalars/0","scalb/2","scalbln/2","scan/1","scan/2","select/1","setpath/2","significand/0","sin/0","sinh/0","sort/0","sort_by/1","split/1","split/2","splits/1","splits/2","sqrt/0","startswith/1","stderr/0","strflocaltime/1","strftime/1","strings/0","strptime/1","sub/2","sub/3","tan/0","tanh/0","test/1","test/2","tgamma/0","to_entries/0","todate/0","todateiso8601/0","tojson/0","tonumber/0","tostream/0","tostring/0","transpose/0","trunc/0","truncate_stream/1","type/0","unique/0","unique_by/1","until/2","utf8bytelength/0","values/0","walk/1","while/2","with_entries/1","y0/0","y1/0","yn/2"];
