const https = require("https");
const ENV_URL = process.env.URL;
const helpers = require("./helpers");

https
  .get(`https://api.hackertarget.com/pagelinks/?q=${ENV_URL}`, resp => {
    let data = "";
    resp.on("data", chunk => {
      data += chunk;
    });

    resp.on("end", () => {
      let cleanUrls = helpers.extractUrls(data, ENV_URL);
      let tasks = helpers.generateTasks(cleanUrls);
      let pythonFile = helpers.generateFile(tasks);
      helpers.extractFile(pythonFile);
    });
  })
  .on("error", err => {
    console.log("Error: " + err.message);
  });
