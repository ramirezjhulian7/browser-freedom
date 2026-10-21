function run() {
  var d = {};
  for (var i = 0; i < 5000; i++) d['key' + i] = i;
  var s = 0;
  for (var r = 0; r < 20; r++) {
    for (var i = 0; i < 5000; i++) s += d['key' + i];
  }
  var keys = Object.keys(d);
  for (var i = 0; i < 2500; i++) delete d['key' + i];
  return s + keys.length + Object.keys(d).length;
}
check(run(), 20 * 12497500 + 5000 + 2500);
