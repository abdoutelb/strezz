const ENV_URL = process.env.url;
const helpers = require("./helpers");
const extracter = require("./extracter");

(async () => {
  try {
    let data = await extracter.extractLinks(ENV_URL);
    let cleanUrls = helpers.extractUrls(data);
    let tasks = helpers.generateTasks(cleanUrls);
    let pythonFile = helpers.generateFile(tasks);
    helpers.extractFile(pythonFile);
  } catch (e) {
    throw new Error(e);
  }
})();
