const ENV_URL = process.env.url;
const helpers = require("./helpers");
const extracter = require("./extracter");

(async () => {
  try {
    const data = await extracter.extractLinks(ENV_URL);
    const cleanUrls = helpers.extractUrls(data);
    const tasks = helpers.generateTasks(cleanUrls);
    const pythonFile = helpers.generateFile(tasks);
    helpers.extractFile(pythonFile);
  } catch (e) {
    throw new Error(e);
  }
})();
