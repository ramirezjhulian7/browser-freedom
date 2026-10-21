function run() {
  var points = [];
  for (var i = 0; i < 100; i++) points.push({ x: i, y: i * 2, z: 0 });
  var total = 0;
  for (var k = 0; k < 3000; k++) {
    for (var i = 0; i < points.length; i++) {
      var p = points[i];
      p.z = p.x + p.y;
      total += p.z;
    }
  }
  return total;
}
check(run(), 3000 * 3 * (99 * 100) / 2);
