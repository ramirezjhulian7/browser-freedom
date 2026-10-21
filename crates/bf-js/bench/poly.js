function area(s) {
  return s.w * s.h + (s.extra || 0);
}
function run() {
  var shapes = [];
  for (var i = 0; i < 50; i++) {
    shapes.push({ w: i, h: 2 });
    shapes.push({ h: 3, w: i });
    shapes.push({ w: 1, h: i, extra: 1 });
    shapes.push({ kind: 'sq', w: i, h: i });
  }
  var total = 0;
  for (var k = 0; k < 2000; k++) {
    for (var j = 0; j < shapes.length; j++) total += area(shapes[j]);
  }
  return total;
}
check(run(), 2000 * (2450 + 3675 + 1225 + 50 + 40425));
