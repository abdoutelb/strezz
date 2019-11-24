const fs = require("fs");


function extractUrls(urls, ENV_URL) {
  let cleanArray = urls.split("\n");
  let usedLinks = [];
  for (let index = 0; index < cleanArray.length; index++) {
    if (cleanArray[index].startsWith(ENV_URL))
      usedLinks.push(encodeURI(cleanArray[index].split(ENV_URL)[1]));
  }
  return usedLinks;
}

function extractFile(python) {
  fs.writeFile("./loc.py", python, "utf8", function(err) {
    if (err) {
      return console.log(err);
    }
    console.log("done !");
  });
}

function generateTasks(links) {
  let result = "";
  for (let index = 1; index <= links.length; index++) {
    result += `
    @task(${index})
    def f${index}(self):
      self.client.get("${links[index]}")
           
      `;
  }
  return result;
}

function generateFile(tasks) {
  return `
from locust import Locust, TaskSet, task, between, HttpLocust


class UserBehavior(TaskSet):

    ${tasks}

class WebsiteUser(HttpLocust):
    task_set = UserBehavior
    wait_time = between(5, 15)

  `;
}

try {
  module.exports = {
    extractUrls: extractUrls,
    extractFile: extractFile,
    generateTasks: generateTasks,
    generateFile: generateFile
  };
} catch (error) {
  console.log("We caught error");
}
