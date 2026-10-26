// Derives the store build numbers from the version in app.json so a new
// release always installs over the previous one: 1.2.3 → 10203. Turns on
// update code signing once certs/certificate.pem is committed, so
// the repository builds before the key exists and refuses unsigned
// updates after it.
const fs = require("fs");
const path = require("path");

const CERTIFICATE = "./certs/certificate.pem";

module.exports = ({ config }) => {
  const [major, minor, patch] = config.version.split(".").map(Number);
  const build = major * 10000 + minor * 100 + patch;
  const signed = fs.existsSync(path.join(__dirname, CERTIFICATE));
  return {
    ...config,
    ios: { ...config.ios, buildNumber: String(build) },
    android: { ...config.android, versionCode: build },
    updates: {
      ...config.updates,
      ...(signed
        ? { codeSigningCertificate: CERTIFICATE, codeSigningMetadata: { keyid: "main", alg: "rsa-v1_5-sha256" } }
        : {}),
    },
  };
};
