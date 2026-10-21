function run() {
  var parts = [];
  for (var i = 0; i < 20000; i++) parts.push('item' + i);
  var s = parts.join(',');
  var count = 0;
  var pos = 0;
  while ((pos = s.indexOf('item1', pos)) !== -1) {
    count++;
    pos += 5;
  }
  var t = '';
  for (var i = 0; i < 5000; i++) t += `${i % 10}`;
  var sliced = 0;
  for (var i = 0; i < 20000; i++) sliced += s.slice(i, i + 4).length;
  return count + t.length + sliced + s.split(',').length;
}
check(run(), 11111 + 5000 + 80000 + 20000);
