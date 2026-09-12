set -eu
d=$(/bin/busybox mktemp -d /tmp/kinakaze-gpg.XXXXXX)
trap '/bin/busybox rm -rf "$d"' EXIT

# 1. Version checks
/usr/bin/gpgv --homedir "$d" --version | /bin/busybox grep -q "gpgv (GnuPG)"
/usr/bin/gpg --homedir "$d" --version | /bin/busybox grep -q "gpg (GnuPG)"

# 2. Materialize keyring and signed message
/bin/busybox printf '%s' "mDMEaqyhbxYJKwYBBAHaRw8BAQdAnsw9dpUxJsiPX2IYd9XyDvlvsGskuRG7OtFV+MNNA8u0FVRlc3QgPHRlc3RAbG9jYWxob3N0PoiTBBMWCgA7FiEEZl1oAiEtwIthVoCc37nFSi9IABEFAmqsoW8CGwMFCwkIBwICIgIGFQoJCAsCBBYCAwECHgcCF4AACgkQ37nFSi9IABGBYwD9HtHkKIqM0S0Od6j4spL2fpum3X8QQ95G9KdhFaIcvfEA/1nWI1ChBIKaC/B2cwsknwxAsffVU9k6VsW7VYC1nWIJ" | /bin/busybox base64 -d > "$d/keyring.gpg"

/bin/busybox printf '%s' "Q1JZU09BQ1VfVkVSSUZJRURfU0lHTkFUVVJFXzIwMjYNCg==" | /bin/busybox base64 -d > "$d/msg.txt"

/bin/busybox printf '%s' "iHUEABYKAB0WIQRmXWgCIS3Ai2FWgJzfucVKL0gAEQUCaqyhcAAKCRDfucVKL0gAEUi/AP95cP+to20RnuwlZjB8cRIYn7GKsBhU90UeskAKkBJfjAD7BWgxnLOafCV+/ktvwtInHMFpPceEPT56QV+E7rf3VQI=" | /bin/busybox base64 -d > "$d/msg.sig"

# 3. gpgv verify good signature
/usr/bin/gpgv --homedir "$d" --keyring "$d/keyring.gpg" "$d/msg.sig" "$d/msg.txt" > "$d/verify.log" 2>&1
/bin/busybox grep -q 'Good signature from "Test <test@localhost>"' "$d/verify.log"

# 4. gpgv reject tampered signature
/bin/busybox printf 'TAMPERED_DATA\n' > "$d/tampered.txt"
if /usr/bin/gpgv --homedir "$d" --keyring "$d/keyring.gpg" "$d/msg.sig" "$d/tampered.txt" >/dev/null 2>&1; then
    echo "gpgv should have rejected tampered file" >&2
    exit 1
fi

# The agent belongs to this one shell and temporary home. GNU daemon command
# mode terminates the agent after the command exits, including failure paths.
/bin/busybox cat > "$d/roundtrip.sh" <<'ROUNDTRIP'
set -eu
d=$1
# 5. gpg symmetric encryption and decryption
printf 'KINAKAZE_SYMMETRIC_PLAINTEXT_12345\n' > "$d/plain.txt"
/usr/bin/gpg --no-autostart --no-symkey-cache --homedir "$d" --batch --yes --pinentry-mode loopback --passphrase "kinakaze-passphrase" --cipher-algo AES256 --symmetric -o "$d/cipher.gpg" "$d/plain.txt"

/usr/bin/gpg --no-autostart --no-symkey-cache --homedir "$d" --batch --yes --pinentry-mode loopback --passphrase "kinakaze-passphrase" --decrypt -o "$d/decrypted.txt" "$d/cipher.gpg"
/bin/busybox cmp "$d/plain.txt" "$d/decrypted.txt"

# 6. gpg reject wrong passphrase
if /usr/bin/gpg --no-autostart --no-symkey-cache --homedir "$d" --batch --yes --pinentry-mode loopback --passphrase "wrong-passphrase" --decrypt "$d/cipher.gpg" >/dev/null 2>&1; then
    echo "gpg decrypt should have failed with wrong passphrase" >&2
    exit 1
fi

ROUNDTRIP
/usr/bin/gpg-agent --homedir "$d" --daemon /bin/sh "$d/roundtrip.sh" "$d"

printf 'GPG_OK\n'
