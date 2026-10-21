function run() {
  var sum = 0;
  for (var i = 0; i < 1000000; i++) {
    if (i % 3 === 0) sum += i;
    else sum -= 1;
  }
  let acc = 0;
  for (let j = 0; j < 300000; j++) {
    acc = (acc + j * 7) | 0;
  }
  return sum + acc;
}
check(run(), 168132504059);
