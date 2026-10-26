// Derives the store build numbers from the version in app.json so a new
// release always installs over the previous one: 1.2.3 → 10203.
module.exports = ({ config }) => {
  const [major, minor, patch] = config.version.split(".").map(Number);
  const build = major * 10000 + minor * 100 + patch;
  return {
    ...config,
    ios: { ...config.ios, buildNumber: String(build) },
    android: { ...config.android, versionCode: build },
  };
};
