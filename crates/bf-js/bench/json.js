function run() {
  var data = [];
  for (var i = 0; i < 2000; i++) {
    data.push({ id: i, name: 'n' + i, tags: ['a', 'b', 'c'], nested: { ok: i % 2 === 0, v: i / 4 } });
  }
  var total = 0;
  for (var k = 0; k < 5; k++) {
    var text = JSON.stringify(data);
    var back = JSON.parse(text);
    total += back.length + back[1999].nested.v + text.length % 1000;
  }
  return total;
}
check(run(), 14203.75);
