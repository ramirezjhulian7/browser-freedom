// Bucle en el código global con `var` (E8): `s` e `i` son propiedades del objeto global, así
// que cada vuelta lee y escribe el objeto global (GetGlobal, ResolveGlobalRef, PutRef).
var s = 0;
for (var i = 0; i < 2000000; i++) {
  s = s + i;
}
check(s, 1999999000000);
