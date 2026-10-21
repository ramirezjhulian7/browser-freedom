function run() {
  var words = ['lorem', 'ipsum', 'dolor', 'sit', 'amet', 'consectetur', 'adipiscing', 'elit', 'sed', 'do'];
  var parts = [];
  for (var i = 0; i < 20000; i++) parts.push(words[i % 10] + (i % 7 === 0 ? ' ' + i + '@mail.test' : ''));
  var text = parts.join(' ');
  var total = 0;
  for (var r = 0; r < 3; r++) {
    var emails = text.match(/[a-z]+ \d+@[a-z]+\.test/g);
    total += emails.length;
    var re = /\b(\w)(\w*)\b/g;
    var m;
    var caps = 0;
    while ((m = re.exec(text)) !== null) if (m[1] === 'l') caps++;
    total += caps;
    total += text.replace(/o/g, '0').length;
    total += text.split(/\s+/).length;
    if (/elit\s+sed\s+do/.test(text)) total++;
  }
  return total;
}
check(run(), 587565);
