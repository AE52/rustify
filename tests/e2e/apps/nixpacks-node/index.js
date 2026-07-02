// Minimal Express app for the Rustify nixpacks E2E fixture.
// Nixpacks auto-detects Node from package.json and runs `npm start`.
const express = require("express");

const app = express();
const port = process.env.PORT || 3000;

app.get("/", (_req, res) => {
  res.send("hello from rustify nixpacks-node");
});

app.listen(port, "0.0.0.0", () => {
  console.log(`nixpacks-node listening on ${port}`);
});
