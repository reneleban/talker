#!/bin/bash
# Legt das selbstsignierte Codesign-Zertifikat »talker-dev« an (einmal pro Mac).
# Damit überleben die TCC-Permissions (Bedienungshilfen/Mikrofon) jeden Rebuild —
# ad-hoc signiert wäre die App für macOS nach jedem `make install` eine neue App.
# Nutzung: make cert  (fragt einmalig nach dem Login-Passwort für die Vertrauens-
# einstellung; das ist die CLI-Entsprechung zum Zertifikatsassistenten).
set -euo pipefail

NAME="talker-dev"

if security find-identity -p codesigning -v | grep -q "\"$NAME\""; then
  echo "✓ Zertifikat »$NAME« existiert bereits — nichts zu tun."
  exit 0
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "→ Schlüssel + selbstsigniertes Codesign-Zertifikat erzeugen …"
openssl req -x509 -newkey rsa:2048 -sha256 -days 3650 -nodes \
  -keyout "$TMP/key.pem" -out "$TMP/cert.pem" \
  -subj "/CN=$NAME" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=critical,codeSigning"

echo "→ In den Login-Schlüsselbund importieren …"
openssl pkcs12 -export -inkey "$TMP/key.pem" -in "$TMP/cert.pem" \
  -out "$TMP/$NAME.p12" -passout pass:talker -name "$NAME"
security import "$TMP/$NAME.p12" \
  -k "$HOME/Library/Keychains/login.keychain-db" \
  -P talker -T /usr/bin/codesign

echo "→ Als vertrauenswürdig fürs Codesigning markieren (macOS fragt nach dem Passwort) …"
security add-trusted-cert -p codeSign \
  -k "$HOME/Library/Keychains/login.keychain-db" "$TMP/cert.pem"

echo "✓ »$NAME« angelegt — ab jetzt signiert »make install« stabil und die"
echo "  Permissions überleben Rebuilds. Nach dem NÄCHSTEN install einmalig die"
echo "  Berechtigungen neu erteilen (die Signatur wechselt von ad-hoc auf $NAME)."
