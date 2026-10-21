function run() {
  var keep = null;
  var n = 0;
  for (var i = 0; i < 200000; i++) {
    var o = { a: i, b: [i, i + 1], c: { d: 'x' } };
    n += o.b[1] - o.a;
    if (i % 1000 === 0) keep = o;
  }
  return n + keep.a;
}
check(run(), 200000 + 199000);
