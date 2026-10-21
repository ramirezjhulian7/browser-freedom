function Body(x, y, z, vx, vy, vz, mass) {
  this.x = x; this.y = y; this.z = z;
  this.vx = vx; this.vy = vy; this.vz = vz;
  this.mass = mass;
}
function advance(bodies, dt) {
  var n = bodies.length;
  for (var i = 0; i < n; i++) {
    var bi = bodies[i];
    for (var j = i + 1; j < n; j++) {
      var bj = bodies[j];
      var dx = bi.x - bj.x, dy = bi.y - bj.y, dz = bi.z - bj.z;
      var d2 = dx * dx + dy * dy + dz * dz;
      var mag = dt / (d2 * Math.sqrt(d2));
      bi.vx -= dx * bj.mass * mag; bi.vy -= dy * bj.mass * mag; bi.vz -= dz * bj.mass * mag;
      bj.vx += dx * bi.mass * mag; bj.vy += dy * bi.mass * mag; bj.vz += dz * bi.mass * mag;
    }
  }
  for (var i = 0; i < n; i++) {
    var b = bodies[i];
    b.x += dt * b.vx; b.y += dt * b.vy; b.z += dt * b.vz;
  }
}
function energy(bodies) {
  var e = 0;
  for (var i = 0; i < bodies.length; i++) {
    var b = bodies[i];
    e += 0.5 * b.mass * (b.vx * b.vx + b.vy * b.vy + b.vz * b.vz);
    for (var j = i + 1; j < bodies.length; j++) {
      var c = bodies[j];
      var dx = b.x - c.x, dy = b.y - c.y, dz = b.z - c.z;
      e -= (b.mass * c.mass) / Math.sqrt(dx * dx + dy * dy + dz * dz);
    }
  }
  return e;
}
function run() {
  var bodies = [
    new Body(0, 0, 0, 0, 0, 0, 39.47),
    new Body(4.84, -1.16, -0.10, 0.60, 2.81, -0.02, 0.037),
    new Body(8.34, 4.12, -0.40, -1.01, 1.82, 0.008, 0.011),
    new Body(12.89, -15.11, -0.22, 1.08, 0.86, -0.01, 0.0017),
    new Body(15.37, -25.91, 0.17, 0.97, 0.59, -0.03, 0.002),
  ];
  for (var i = 0; i < 20000; i++) advance(bodies, 0.01);
  return Math.round(energy(bodies) * 1e6);
}
check(run(), -166593);
