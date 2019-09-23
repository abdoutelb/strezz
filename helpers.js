const fs = require("fs");

function isArabic(text) {
  var pattern = /[\u0600-\u06FF\u0750-\u077F]/;
  result = pattern.test(text);
  return result;
}

function extractUrls(urls, ENV_URL) {
  let cleanArray = urls.split("\n");
  let usedLinks = [];
  for (let index = 0; index < cleanArray.length; index++) {
    if (cleanArray[index].startsWith(ENV_URL))
      usedLinks.push(encodeURI(cleanArray[index].split(ENV_URL)[1]));
  }
  return usedLinks;
}

try {
  module.exports = {
    isArabic: isArabic,
    extractUrls: extractUrls,
    extractFile: extractFile,
    generateTasks: generateTasks,
    generateFile: generateFile
  };
} catch (error) {
  console.log("We caught error");
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
    result += `@task(${index})
      def f${index}(self):
       self.client.get("${links[index]}")
           
      `;
  }
  return result;
}

function generateFile(tasks) {
  return `
  from locust import HttpLocust, TaskSet, task
  
  
  class UserBehavior(TaskSet):
  
      def fon_start(self):
          self.login()
  
      def flogin(self):
          self.client.get("/")
  
      
      ${tasks}
  
  class WebsiteUser(HttpLocust):
      task_set = UserBehavior
      min_wait = 5000
      max_wait = 9000
  
    `;
}
