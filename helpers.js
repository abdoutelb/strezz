const fs = require("fs");

function extractUrls(urls) {
  let cleanArray = urls;
  let usedLinks = [];
  for (let index = 0; index < cleanArray.length; index++) {
    if (cleanArray[index] !== undefined && cleanArray[index].startsWith("/"))
      usedLinks.push(encodeURI(cleanArray[index]));
  }
  return usedLinks;
}

function extractFile(python) {
  fs.writeFile("./loc.py", python, "utf8", function (err) {
    if (err) {
      throw new Error(err);
    }
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
import random
from locust import HttpUser, task, between


class QuickstartUser(HttpUser):
    wait_time = between(5, 9)

    ${tasks}

  `;
}

try {
  module.exports = {
    extractUrls: extractUrls,
    extractFile: extractFile,
    generateTasks: generateTasks,
    generateFile: generateFile,
  };
} catch (error) {
  throw new Error("We caught error");
}
