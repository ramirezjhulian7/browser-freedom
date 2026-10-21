function counter(start) {
  var n = start;
  return {
    inc: function () { n += 1; return n; },
    get: function () { return n; },
  };
}
function run() {
  var cs = [];
  for (var i = 0; i < 10; i++) cs.push(counter(i));
  var total = 0;
  for (var k = 0; k < 30000; k++) {
    for (var i = 0; i < cs.length; i++) total += cs[i].inc() & 1;
  }
  var adders = [];
  for (var j = 0; j < 100; j++) adders.push((x) => x + j);
  var s = 0;
  for (var r = 0; r < 1000; r++) for (var j = 0; j < adders.length; j++) s = adders[j](s) % 1000003;
  return total + s + cs[3].get();
}
check(run(), 150000 + 4950000 % 1000003 + 30003);
