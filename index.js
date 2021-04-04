const ENV_URL = process.env.url;
const helpers = require("./helpers");
const extracter = require("./extracter");

extracter.extractLinks(ENV_URL).then((data) => {
  let cleanUrls = helpers.extractUrls(data, ENV_URL);
  let tasks = helpers.generateTasks(cleanUrls);
  let pythonFile = helpers.generateFile(tasks);
  helpers.extractFile(pythonFile);
});
