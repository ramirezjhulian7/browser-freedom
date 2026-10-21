function run() {
  var s = '';
  for (var i = 0; i < 100000; i++) s += 'x' + (i % 10);
  var t = '';
  for (var i = 0; i < 20000; i++) t += `<li id="${i}">${i * 2}</li>`;
  var u = '';
  for (var i = 0; i < 2000; i++) u = u + String.fromCharCode(97 + i % 26);
  return s.length + t.length + u.length + s.charCodeAt(s.length - 1) + t.indexOf('id="19999"');
}
check(run(), 1168706);
