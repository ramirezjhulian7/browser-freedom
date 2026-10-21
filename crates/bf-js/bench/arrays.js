function run() {
  var a = [];
  for (var i = 0; i < 100000; i++) a.push(i);
  var s = 0;
  for (var i = 0; i < a.length; i++) s += a[i];
  var b = a.map(function (x) { return x * 2; }).filter(function (x) { return x % 3 === 0; });
  var r = b.reduce(function (acc, x) { return acc + x; }, 0);
  for (var k = 0; k < 20; k++) {
    var c = a.slice(0, 1000);
    c.reverse();
    s += c[0];
  }
  return s + r + b.length;
}
check(run(), 8333369980);
