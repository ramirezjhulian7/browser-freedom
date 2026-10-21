class Vec2 {
  constructor(x, y) {
    this.x = x;
    this.y = y;
  }
  add(o) {
    return new Vec2(this.x + o.x, this.y + o.y);
  }
  dot(o) {
    return this.x * o.x + this.y * o.y;
  }
  get len2() {
    return this.dot(this);
  }
}
function run() {
  var acc = new Vec2(0, 0);
  var step = new Vec2(1, 2);
  var s = 0;
  for (var i = 0; i < 100000; i++) {
    acc = acc.add(step);
    s += acc.dot(step) % 7;
  }
  return s + acc.len2;
}
check(run(), 50000300004);
